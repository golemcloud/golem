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

use super::index::{
    logical_payloads, nested_coordinate_matches_item, registration_coordinate_depth,
    validate_items_payload,
};
use super::publication::{STREAM_SEGMENT_MAX_EVENTS, STREAM_SEGMENT_TARGET_BYTES};
use super::registration::{registration_matches, registration_record};
use super::terminals::fenced_by_terminal;
use super::*;

/// Latest committed position and terminal state of a durable stream.
pub struct StreamHead {
    pub offset: Option<StreamOffset>,
    pub closed: bool,
    pub cancelled: bool,
}

pub(super) struct AppliedWriteBatch {
    pub(super) events: Vec<CommittedProducerStreamEvent>,
    pub(super) newly_registered_stream_count: usize,
}

impl DurableStreamStore {
    #[cfg(test)]
    /// Appends values in producer order and returns only after their durable receipt.
    pub(crate) async fn write_items(
        &self,
        context: Option<&StreamWriteContext>,
        stream_id: StreamId,
        first_sequence: u64,
        payload: StreamItemsPayload,
    ) -> Result<ProducerWriteOutcome<Vec<StreamOffset>>, StreamStoreError> {
        self.write_items_with_nested(context, stream_id, first_sequence, payload, Vec::new())
            .await
    }

    /// Resolves a registration coordinate to its fingerprint-bound durable handle.
    pub async fn handle_for_coordinate(
        &self,
        coordinate: &StreamRegistrationCoordinate,
    ) -> Result<Option<DurableStreamHandle>, StreamStoreError> {
        let index = self
            .index_for([ProducerMetadataKey::Coordinate(coordinate.clone())])
            .await?;
        Ok(index
            .coordinates
            .get(coordinate)
            .and_then(|stream_id| index.registrations.get(stream_id))
            .map(|registration| registration.handle.clone()))
    }

    /// Returns the latest committed offset and terminal state for a validated handle.
    pub async fn stream_head(
        &self,
        handle: &DurableStreamHandle,
    ) -> Result<StreamHead, StreamStoreError> {
        let index = self.index_for_terminal([], handle.stream_id).await?;
        if index
            .registrations
            .get(&handle.stream_id)
            .is_none_or(|registration| &registration.handle != handle)
        {
            return Err(StreamStoreError::InvalidHandle);
        }
        let stream = &index.streams[&handle.stream_id];
        let cancelled = stream.terminal_event.as_ref().is_some_and(|event| {
            matches!(
                event.payload,
                CommittedProducerStreamEventPayload::Cancel { .. }
            )
        });
        Ok(StreamHead {
            offset: stream.last_offset,
            closed: stream.terminal,
            cancelled,
        })
    }

    /// Reads committed events by offset; resident publication state is not authoritative.
    pub async fn read_by_handle(
        self: &Arc<Self>,
        request: golem_common::model::durable_stream::StreamHandleReadRequest,
    ) -> Result<StreamHandleReadResult, StreamStoreError> {
        if let Some(offset) = request.after {
            StreamOffset::from_bytes(offset.0)
                .map_err(|error| StreamStoreError::InvalidOffset(error.to_string()))?;
        }
        let StreamHead {
            offset: mut head,
            mut closed,
            mut cancelled,
        } = self.stream_head(&request.handle).await?;
        if request.max_items == 0 {
            return Ok(StreamHandleReadResult {
                events: Vec::new(),
                next_offset: request.after,
                head_offset: head,
                closed,
                cancelled,
            });
        }
        if request.max_bytes == 0 {
            return Err(StreamStoreError::InvalidValueBatch);
        }
        let mut events = if head.is_some() && request.after <= head {
            self.read_segment(&request.handle, request.after, head)
                .await?
        } else {
            Vec::new()
        };
        if events.is_empty() && !closed && request.wait_millis > 0 {
            let bus = self.stream_bus(request.handle.stream_id).await?;
            let deadline = tokio::time::Instant::now()
                + std::time::Duration::from_millis(request.wait_millis.min(30_000));
            loop {
                let changed = bus.high_water_changed();
                bus.ensure_available()?;
                let stream_head = self.stream_head(&request.handle).await?;
                head = stream_head.offset;
                closed = stream_head.closed;
                cancelled = stream_head.cancelled;
                events = if head.is_some() && request.after <= head {
                    self.read_segment(&request.handle, request.after, head)
                        .await?
                } else {
                    Vec::new()
                };
                bus.ensure_available()?;
                if !events.is_empty() || closed || tokio::time::Instant::now() >= deadline {
                    break;
                }
                if let Ok(result) = tokio::time::timeout_at(deadline, changed).await {
                    result?;
                }
            }
        }
        let max_items = (request.max_items as usize).min(STREAM_SEGMENT_MAX_EVENTS);
        let max_bytes = request.max_bytes.min(STREAM_SEGMENT_TARGET_BYTES as u64);
        let mut bytes = 0u64;
        let mut count = 0usize;
        for event in &events {
            let size = match &event.payload {
                CommittedProducerStreamEventPayload::Value(value) => value.len() as u64,
                CommittedProducerStreamEventPayload::PackedU8(_) => 1,
                _ => 0,
            };
            if count == max_items || bytes + size > max_bytes {
                if count == 0 {
                    return Err(StreamStoreError::InvalidValueBatch);
                }
                break;
            }
            bytes += size;
            count += 1;
        }
        events.truncate(count);
        let next_offset = events.last().map(|event| event.offset).or(request.after);
        Ok(StreamHandleReadResult {
            events,
            next_offset,
            head_offset: head,
            closed,
            cancelled,
        })
    }

    /// Resolves nested stream handles carried by the selected committed item range.
    pub async fn nested_handles(
        &self,
        stream_id: StreamId,
        first_sequence: u64,
    ) -> Result<Vec<DurableStreamHandle>, StreamStoreError> {
        let index = self
            .index_for([ProducerMetadataKey::Batch(stream_id, first_sequence)])
            .await?;
        let stream = index
            .streams
            .get(&stream_id)
            .ok_or(StreamStoreError::UnknownStream(stream_id))?;
        let oplog_index = *stream
            .batches
            .get(&first_sequence)
            .ok_or(StreamStoreError::EventConflict)?;
        drop(index);
        let record = self.read_item_batch(oplog_index).await?;
        self.resolve_nested_handles(&record.nested_stream_ids).await
    }

    pub(super) async fn read_result_record(
        &self,
        oplog_index: OplogIndex,
    ) -> Result<StreamSessionRecord, StreamStoreError> {
        let OplogEntry::StreamSession { record, .. } = self.oplog.read(oplog_index).await else {
            return Err(StreamStoreError::CorruptHistory(
                "result metadata points at a non-session record".into(),
            ));
        };
        self.oplog
            .download_payload(record)
            .await
            .map_err(StreamStoreError::Oplog)
    }

    pub(super) async fn read_item_batch(
        &self,
        oplog_index: OplogIndex,
    ) -> Result<StreamItemsRecord, StreamStoreError> {
        let OplogEntry::StreamItems { record, .. } = self.oplog.read(oplog_index).await else {
            return Err(StreamStoreError::CorruptHistory(
                "stream batch index points at a non-item record".into(),
            ));
        };
        self.oplog
            .download_payload(record)
            .await
            .map_err(StreamStoreError::Oplog)
    }

    pub(super) async fn resolve_nested_handles(
        &self,
        stream_ids: &[StreamId],
    ) -> Result<Vec<DurableStreamHandle>, StreamStoreError> {
        if stream_ids.is_empty() {
            return Ok(Vec::new());
        }
        let index = self
            .index_for(
                stream_ids
                    .iter()
                    .flat_map(|id| {
                        [
                            ProducerMetadataKey::Stream(*id),
                            ProducerMetadataKey::Reference(*id),
                        ]
                    })
                    .collect::<Vec<_>>(),
            )
            .await?;
        stream_ids
            .iter()
            .map(|stream_id| {
                index
                    .registrations
                    .get(stream_id)
                    .map(|registration| registration.handle.clone())
                    .or_else(|| {
                        index
                            .referenced_handles
                            .get(stream_id)
                            .map(|(handle, _)| handle.clone())
                    })
                    .ok_or(StreamStoreError::UnknownStream(*stream_id))
            })
            .collect()
    }

    async fn materialize_item_batch(
        &self,
        record: &StreamItemsRecord,
    ) -> Result<Vec<CommittedProducerStreamEvent>, StreamStoreError> {
        self.materialize_item_batch_range(record, record.first_sequence, usize::MAX)
            .await
    }

    pub(super) async fn materialize_item_batch_range(
        &self,
        record: &StreamItemsRecord,
        first_sequence: u64,
        limit: usize,
    ) -> Result<Vec<CommittedProducerStreamEvent>, StreamStoreError> {
        let nested_handles = self
            .resolve_nested_handles(&record.nested_stream_ids)
            .await?;
        let packed_u8_batch_end = matches!(record.payload, StreamItemsPayload::PackedU8(_))
            .then(|| record.offsets.last().copied())
            .flatten();
        logical_payloads(&record.payload)
            .zip(&record.offsets)
            .enumerate()
            .skip(
                first_sequence
                    .saturating_sub(record.first_sequence)
                    .try_into()
                    .unwrap_or(usize::MAX),
            )
            .take(limit)
            .map(|(sub_index, (payload, offset))| {
                Ok(CommittedProducerStreamEvent {
                    stream_id: record.stream_id,
                    producer_sequence: record
                        .first_sequence
                        .checked_add(sub_index as u64)
                        .ok_or(StreamStoreError::CounterOverflow)?,
                    offset: *offset,
                    packed_u8_batch_end,
                    terminal_author: None,
                    nested_handles: nested_handles.clone(),
                    payload,
                })
            })
            .collect()
    }

    /// Returns the committed producer sequence through which an input stream may resume.
    pub async fn input_high_water(
        &self,
        stream_id: StreamId,
    ) -> Result<Option<InputStreamHighWater>, StreamStoreError> {
        let index = self
            .index_for([ProducerMetadataKey::Stream(stream_id)])
            .await?;
        let stream = index
            .streams
            .get(&stream_id)
            .ok_or(StreamStoreError::UnknownStream(stream_id))?;
        let Some(resulting_offset) = stream.last_offset else {
            return Ok(None);
        };
        Ok(Some(InputStreamHighWater {
            highest_contiguous_sequence: if stream.terminal {
                stream.next_sequence
            } else {
                stream.next_sequence - 1
            },
            resulting_offset,
            terminal: stream.terminal,
        }))
    }

    /// Returns the input high-water mark while validating the consumer attachment.
    pub async fn attached_input_high_water(
        &self,
        session_key: &StreamSessionKey,
        stream_id: StreamId,
    ) -> Result<Option<InputStreamHighWater>, StreamStoreError> {
        let index = self
            .index_for_terminal(
                [
                    ProducerMetadataKey::Stream(stream_id),
                    ProducerMetadataKey::ExternalProducerHead(
                        session_key.clone(),
                        stream_id,
                        ExternalProducerId::Attached,
                    ),
                ],
                stream_id,
            )
            .await?;
        let stream = index
            .streams
            .get(&stream_id)
            .ok_or(StreamStoreError::UnknownStream(stream_id))?;
        let head = index.external_producer_heads.get(&(
            session_key.clone(),
            stream_id,
            ExternalProducerId::Attached,
        ));
        let next_sequence = head.map_or(0, |head| head.next_sequence);
        if next_sequence == 0 && !stream.terminal {
            return Ok(None);
        }
        let resulting_offset = stream.last_offset.ok_or_else(|| {
            StreamStoreError::CorruptHistory(
                "terminal attached input has no resulting offset".into(),
            )
        })?;
        Ok(Some(InputStreamHighWater {
            highest_contiguous_sequence: if stream.terminal
                && head.is_none_or(|head| head.last_offset != resulting_offset)
            {
                next_sequence
            } else {
                next_sequence.saturating_sub(1)
            },
            resulting_offset,
            terminal: stream.terminal,
        }))
    }

    #[cfg(test)]
    /// Commits values and their nested stream registrations as one ordered producer mutation.
    pub(crate) async fn write_items_with_nested(
        &self,
        context: Option<&StreamWriteContext>,
        stream_id: StreamId,
        first_sequence: u64,
        payload: StreamItemsPayload,
        nested: Vec<ProducerRegistrationRequest>,
    ) -> Result<ProducerWriteOutcome<Vec<StreamOffset>>, StreamStoreError> {
        self.write_items_with_nested_sources_at_depth(
            context,
            stream_id,
            first_sequence,
            payload,
            nested
                .into_iter()
                .map(NestedStreamWrite::Register)
                .collect(),
            0,
        )
        .await
    }

    #[cfg(test)]
    /// Writes nested values while enforcing the protocol traversal-depth limit.
    pub(crate) async fn write_items_with_nested_at_depth(
        &self,
        context: Option<&StreamWriteContext>,
        stream_id: StreamId,
        first_sequence: u64,
        payload: StreamItemsPayload,
        nested: Vec<ProducerRegistrationRequest>,
        traversal_depth: usize,
    ) -> Result<ProducerWriteOutcome<Vec<StreamOffset>>, StreamStoreError> {
        self.write_items_with_nested_sources_at_depth(
            context,
            stream_id,
            first_sequence,
            payload,
            nested
                .into_iter()
                .map(NestedStreamWrite::Register)
                .collect(),
            traversal_depth,
        )
        .await
    }

    /// Persists values together with newly registered or forwarded nested stream sources.
    pub async fn write_items_with_nested_sources(
        &self,
        context: Option<&StreamWriteContext>,
        stream_id: StreamId,
        first_sequence: u64,
        payload: StreamItemsPayload,
        nested: Vec<NestedStreamWrite>,
    ) -> Result<ProducerWriteOutcome<Vec<StreamOffset>>, StreamStoreError> {
        self.write_items_with_nested_sources_at_depth(
            context,
            stream_id,
            first_sequence,
            payload,
            nested,
            0,
        )
        .await
    }

    #[tracing::instrument(
        name = "durable_stream.write",
        skip_all,
        fields(stream_id = %stream_id, first_sequence)
    )]
    async fn write_items_with_nested_sources_at_depth(
        &self,
        context: Option<&StreamWriteContext>,
        stream_id: StreamId,
        first_sequence: u64,
        payload: StreamItemsPayload,
        nested_sources: Vec<NestedStreamWrite>,
        traversal_depth: usize,
    ) -> Result<ProducerWriteOutcome<Vec<StreamOffset>>, StreamStoreError> {
        let memory = Self::retained_payload_bytes(&payload)?;
        self.run_owned(context, memory, move |owner, context| async move {
            owner
                .write_items_owned(
                    &context,
                    stream_id,
                    first_sequence,
                    payload,
                    nested_sources,
                    traversal_depth,
                    None,
                    None,
                )
                .await
        })
        .await
    }

    /// Writes forwarded values after checking that the source attachment is still active.
    pub async fn write_attached_items_with_nested(
        self: &Arc<Self>,
        context: Option<&StreamWriteContext>,
        session_key: &StreamSessionKey,
        stream_id: StreamId,
        transport_first_sequence: u64,
        payload: StreamItemsPayload,
        nested: Vec<ProducerRegistrationRequest>,
    ) -> Result<ProducerWriteOutcome<Vec<StreamOffset>>, StreamStoreError> {
        let memory = Self::retained_payload_bytes(&payload)?;
        let _transport_next_sequence = transport_first_sequence
            .checked_add(payload.logical_item_count() as u64)
            .ok_or(StreamStoreError::CounterOverflow)?;
        let session_key = session_key.clone();
        self.run_owned(context, memory, move |owner, context| async move {
            let id = ExternalProducerId::Attached;
            let index = owner
                .index_for_terminal(
                    [
                        ProducerMetadataKey::Stream(stream_id),
                        ProducerMetadataKey::ExternalProducerHead(
                            session_key.clone(),
                            stream_id,
                            id.clone(),
                        ),
                        ProducerMetadataKey::ExternalProducerSequence(
                            session_key.clone(),
                            stream_id,
                            id.clone(),
                            0,
                            transport_first_sequence,
                        ),
                    ],
                    stream_id,
                )
                .await?;
            let identity = (session_key.clone(), stream_id, id.clone());
            let stream = index
                .streams
                .get(&stream_id)
                .ok_or(StreamStoreError::UnknownStream(stream_id))?;
            let (global_first_sequence, index) =
                if let Some(head) = index.external_producer_heads.get(&identity) {
                    if transport_first_sequence < head.next_sequence {
                        let offset = index
                            .external_producer_offsets
                            .get(&(
                                identity.0,
                                identity.1,
                                identity.2,
                                0,
                                transport_first_sequence,
                            ))
                            .copied()
                            .ok_or_else(|| {
                                StreamStoreError::CorruptHistory(
                                    "attached producer sequence has no original offset".into(),
                                )
                            })?;
                        if stream.terminal && stream.last_offset == Some(offset) {
                            return Err(StreamStoreError::EventConflict);
                        }
                        drop(index);
                        let sequence = owner
                            .read_item_batch(offset.producer_oplog_index())
                            .await?
                            .first_sequence;
                        (sequence, None)
                    } else {
                        if stream.terminal {
                            return Err(if Some(head.last_offset) == stream.last_offset {
                                fenced_by_terminal(stream)
                            } else {
                                StreamStoreError::ClosedByOtherProducer
                            });
                        }
                        if transport_first_sequence != head.next_sequence {
                            return Err(StreamStoreError::SequenceGap {
                                expected: head.next_sequence,
                                actual: transport_first_sequence,
                            });
                        }
                        (stream.next_sequence, Some(index))
                    }
                } else {
                    if stream.terminal {
                        return Err(StreamStoreError::ClosedByOtherProducer);
                    }
                    if transport_first_sequence != 0 {
                        return Err(StreamStoreError::SequenceGap {
                            expected: 0,
                            actual: transport_first_sequence,
                        });
                    }
                    (stream.next_sequence, Some(index))
                };
            let delta = global_first_sequence
                .checked_sub(transport_first_sequence)
                .ok_or_else(|| {
                    StreamStoreError::CorruptHistory(
                        "attached transport sequence exceeds global stream sequence".into(),
                    )
                })?;
            let nested = nested
                .into_iter()
                .map(|mut request| {
                    if let StreamRegistrationCoordinate::Nested {
                        parent_producer_sequence,
                        ..
                    } = &mut request.coordinate
                    {
                        *parent_producer_sequence = parent_producer_sequence
                            .checked_add(delta)
                            .ok_or(StreamStoreError::CounterOverflow)?;
                    }
                    Ok(request)
                })
                .collect::<Result<Vec<_>, StreamStoreError>>()?;
            owner
                .write_items_owned(
                    &context,
                    stream_id,
                    global_first_sequence,
                    payload,
                    nested
                        .into_iter()
                        .map(NestedStreamWrite::Register)
                        .collect(),
                    0,
                    Some(ExternalProducer {
                        id,
                        epoch: 0,
                        sequence: transport_first_sequence,
                    }),
                    index,
                )
                .await
        })
        .await
    }

    /// Resolves an attached source offset to its committed producer sequence.
    pub async fn attached_global_sequence(
        &self,
        session_key: &StreamSessionKey,
        stream_id: StreamId,
        transport_sequence: u64,
    ) -> Result<u64, StreamStoreError> {
        let index = self
            .index_for_terminal(
                [
                    ProducerMetadataKey::Stream(stream_id),
                    ProducerMetadataKey::ExternalProducerSequence(
                        session_key.clone(),
                        stream_id,
                        ExternalProducerId::Attached,
                        0,
                        transport_sequence,
                    ),
                ],
                stream_id,
            )
            .await?;
        if let Some(offset) = index
            .external_producer_offsets
            .get(&(
                session_key.clone(),
                stream_id,
                ExternalProducerId::Attached,
                0,
                transport_sequence,
            ))
            .copied()
        {
            drop(index);
            return Ok(self
                .read_item_batch(offset.producer_oplog_index())
                .await?
                .first_sequence);
        }
        index
            .streams
            .get(&stream_id)
            .map(|stream| stream.next_sequence)
            .ok_or(StreamStoreError::UnknownStream(stream_id))
    }

    async fn write_items_owned(
        &self,
        context: &StreamWriteContext,
        stream_id: StreamId,
        first_sequence: u64,
        payload: StreamItemsPayload,
        nested_sources: Vec<NestedStreamWrite>,
        traversal_depth: usize,
        external_producer: Option<ExternalProducer>,
        index: Option<MutexGuard<'_, ProducerStreamIndex>>,
    ) -> Result<ProducerWriteOutcome<Vec<StreamOffset>>, StreamStoreError> {
        validate_items_payload(&payload)?;
        let item_count = payload.logical_item_count() as u64;
        let nested = nested_sources
            .iter()
            .filter_map(|source| match source {
                NestedStreamWrite::Register(request) => Some(request.clone()),
                NestedStreamWrite::Forward(_) => None,
            })
            .collect::<Vec<_>>();

        let mut keys = vec![ProducerMetadataKey::Stream(stream_id)];
        for source in &nested_sources {
            match source {
                NestedStreamWrite::Register(request) => {
                    keys.extend(ProducerMetadataKey::registration(request))
                }
                NestedStreamWrite::Forward(handle) => {
                    keys.push(ProducerMetadataKey::Stream(handle.stream_id));
                    keys.push(ProducerMetadataKey::Reference(handle.stream_id));
                }
            }
        }
        keys.push(ProducerMetadataKey::Batch(stream_id, first_sequence));
        let mut index = if let Some(mut index) = index {
            // Attached writers retain the guard from global sequence assignment
            // through commit, so HTTP writers cannot take that sequence meanwhile.
            self.load_index_keys(&mut index, keys).await?;
            index
        } else {
            self.index_for_terminal(keys, stream_id).await?
        };
        let entity_parent_start_index = index.entity_parent_start_index(stream_id)?;
        if nested
            .iter()
            .any(|request| request.entity_parent_start_index != entity_parent_start_index)
        {
            return Err(StreamStoreError::RegistrationDivergence);
        }
        let session_key = index
            .stream_sessions
            .get(&stream_id)
            .cloned()
            .ok_or(StreamStoreError::UnknownStream(stream_id))?;
        let session_finished = index.finished_sessions.contains(&session_key);
        let stream = index
            .streams
            .get(&stream_id)
            .ok_or(StreamStoreError::UnknownStream(stream_id))?;
        if first_sequence < stream.next_sequence {
            if let Some(oplog_index) = stream.batches.get(&first_sequence).copied() {
                drop(index);
                let record = self.read_item_batch(oplog_index).await?;
                let index = self
                    .index_for(
                        record
                            .nested_stream_ids
                            .iter()
                            .map(|id| ProducerMetadataKey::Stream(*id))
                            .collect::<Vec<_>>(),
                    )
                    .await?;
                let matches = record.payload == payload
                    && record.nested_stream_ids.len() == nested_sources.len()
                    && record.nested_stream_ids.iter().zip(&nested_sources).all(
                        |(stream_id, source)| match source {
                            NestedStreamWrite::Register(request) => index
                                .registrations
                                .get(stream_id)
                                .is_some_and(|record| registration_matches(record, request)),
                            NestedStreamWrite::Forward(handle) => stream_id == &handle.stream_id,
                        },
                    );
                drop(index);
                if !matches {
                    return Err(StreamStoreError::EventConflict);
                }
                let events = self.materialize_item_batch(&record).await?;
                self.publish_repair(Some(context), stream_id, events)
                    .await?;
                crate::metrics::durable_stream::record_producer_operation("write", true);
                tracing::debug!(
                    stream_id = %stream_id,
                    first_sequence,
                    logical_item_count = item_count,
                    replayed = true,
                    "Durable stream item batch resolved"
                );
                return Ok(ProducerWriteOutcome {
                    value: record.offsets,
                    replayed: true,
                });
            }
            if let Some(terminal) = &stream.terminal_event
                && terminal.producer_sequence == first_sequence
            {
                let terminal = terminal.clone();
                let error = fenced_by_terminal(stream);
                drop(index);
                self.publish_repair(Some(context), stream_id, vec![terminal])
                    .await?;
                return Err(error);
            }
            return Err(StreamStoreError::EventConflict);
        }
        index.ensure_producer_write_allowed()?;
        if session_finished {
            return Err(StreamStoreError::SessionFinished(session_key));
        }
        if stream.terminal {
            let terminal = stream
                .terminal_event
                .as_ref()
                .expect("terminal stream has no terminal event")
                .clone();
            let error = fenced_by_terminal(stream);
            drop(index);
            self.publish_repair(Some(context), stream_id, vec![terminal])
                .await?;
            return Err(error);
        }
        if first_sequence != stream.next_sequence {
            return Err(StreamStoreError::SequenceGap {
                expected: stream.next_sequence,
                actual: first_sequence,
            });
        }
        if traversal_depth > MAX_STREAM_VALUE_TRAVERSAL_DEPTH
            || nested.iter().any(|request| {
                registration_coordinate_depth(&request.coordinate)
                    > MAX_STREAM_VALUE_TRAVERSAL_DEPTH
            })
        {
            self.commit_resource_exhausted_terminal(context, index, stream_id, first_sequence)
                .await?;
            crate::metrics::durable_stream::record_limit_violation("traversal_depth");
            return Err(StreamStoreError::TraversalDepthLimit);
        }
        if nested_sources.len() > MAX_NEW_STREAM_HANDLES_PER_VALUE {
            self.commit_resource_exhausted_terminal(context, index, stream_id, first_sequence)
                .await?;
            crate::metrics::durable_stream::record_limit_violation("streams_per_value");
            return Err(StreamStoreError::ValueStreamLimit);
        }
        if (matches!(payload, StreamItemsPayload::PackedU8(_)) && !nested_sources.is_empty())
            || nested.iter().any(|request| {
                !nested_coordinate_matches_item(
                    &request.coordinate,
                    stream_id,
                    first_sequence,
                    item_count,
                )
            })
        {
            return Err(StreamStoreError::RegistrationDivergence);
        }
        for source in &nested_sources {
            if let NestedStreamWrite::Forward(handle) = source
                && (handle.format_version != DURABLE_STREAM_FORMAT_VERSION
                    || index.referenced_handles.get(&handle.stream_id).is_none_or(
                        |(referenced_handle, referenced_sessions)| {
                            referenced_handle != handle
                                || !referenced_sessions.contains(&session_key)
                        },
                    ))
            {
                return Err(StreamStoreError::InvalidHandle);
            }
        }
        let mut seen_coordinates = HashSet::with_capacity(nested.len());
        let mut existing_nested = HashMap::new();
        let mut new_nested = Vec::new();
        for request in &nested {
            if !seen_coordinates.insert(request.coordinate.clone()) {
                return Err(StreamStoreError::RegistrationDivergence);
            }
            if let Some(existing_id) = index.coordinates.get(&request.coordinate) {
                let existing = index
                    .registrations
                    .get(existing_id)
                    .expect("coordinate index points at a missing registration");
                if !registration_matches(existing, request) {
                    return Err(StreamStoreError::RegistrationDivergence);
                }
                existing_nested.insert(request.coordinate.clone(), *existing_id);
            } else {
                new_nested.push(request.clone());
            }
        }
        if new_nested.len() > MAX_NEW_STREAM_HANDLES_PER_VALUE {
            self.commit_resource_exhausted_terminal(context, index, stream_id, first_sequence)
                .await?;
            crate::metrics::durable_stream::record_limit_violation("streams_per_value");
            return Err(StreamStoreError::ValueStreamLimit);
        }
        let mut new_streams_by_session = HashMap::<StreamSessionKey, usize>::new();
        for request in &new_nested {
            let session_key = index
                .registration_session_key(&request.coordinate, &request.session_mapping)
                .ok_or_else(|| match &request.coordinate {
                    StreamRegistrationCoordinate::Nested {
                        parent_stream_id, ..
                    } => StreamStoreError::UnknownStream(*parent_stream_id),
                    StreamRegistrationCoordinate::Root { .. } => {
                        unreachable!("root registration always defines its session")
                    }
                })?;
            *new_streams_by_session.entry(session_key).or_default() += 1;
        }
        let session_limit_exceeded =
            new_streams_by_session
                .into_iter()
                .any(|(session_key, new_stream_count)| {
                    index
                        .session_stream_counts
                        .get(&session_key)
                        .copied()
                        .unwrap_or_default()
                        .checked_add(new_stream_count)
                        .is_none_or(|count| count > MAX_DURABLE_STREAMS_PER_SESSION)
                });
        if session_limit_exceeded {
            self.commit_resource_exhausted_terminal(context, index, stream_id, first_sequence)
                .await?;
            crate::metrics::durable_stream::record_limit_violation("streams_per_session");
            return Err(StreamStoreError::StreamLimit);
        }
        if first_sequence.checked_add(item_count).is_none() {
            self.commit_resource_exhausted_terminal(context, index, stream_id, first_sequence)
                .await?;
            crate::metrics::durable_stream::record_limit_violation("sequence");
            return Err(StreamStoreError::CounterOverflow);
        }
        let ingress_session_key = index
            .registrations
            .get(&stream_id)
            .filter(|registration| {
                registration.source_kind == StreamSourceKind::ExternalInlineInput
            })
            .and_then(|registration| registration.session_mapping.as_ref())
            .map(|mapping| mapping.session_key.clone());

        let environment_id = self.environment_id;
        let producer = self.producer.clone();
        let producer_fingerprint = self.producer_fingerprint;
        let payload_for_entry = payload.clone();
        let registrations_for_entry = new_nested;
        let nested_for_entry = nested_sources;
        let external_session_key = session_key.clone();
        context.begin_durable_effect();
        let entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |first_index| {
                let registration_count = registrations_for_entry.len();
                let mut records = Vec::with_capacity(
                    registration_count + 1 + usize::from(ingress_session_key.is_some()),
                );
                let mut newly_registered_stream_ids = Vec::with_capacity(registration_count);
                let mut newly_registered_by_coordinate = HashMap::with_capacity(registration_count);
                for (position, request) in registrations_for_entry.into_iter().enumerate() {
                    let oplog_index = OplogIndex::from_u64(first_index.as_u64() + position as u64);
                    let registration = registration_record(
                        oplog_index,
                        environment_id,
                        producer.clone(),
                        producer_fingerprint,
                        request,
                    );
                    newly_registered_stream_ids.push(registration.handle.stream_id);
                    newly_registered_by_coordinate.insert(
                        registration.coordinate.clone(),
                        registration.handle.stream_id,
                    );
                    records.push(DurableStreamOplogRecord::Registered(
                        entity_parent_start_index,
                        registration,
                    ));
                }
                let nested_stream_ids = nested_for_entry
                    .iter()
                    .map(|source| match source {
                        NestedStreamWrite::Register(request) => existing_nested
                            .get(&request.coordinate)
                            .or_else(|| newly_registered_by_coordinate.get(&request.coordinate))
                            .copied()
                            .expect(
                                "every validated nested stream is existing or newly registered",
                            ),
                        NestedStreamWrite::Forward(handle) => handle.stream_id,
                    })
                    .collect();
                let item_index =
                    OplogIndex::from_u64(first_index.as_u64() + registration_count as u64);
                let offsets = (0..payload_for_entry.logical_item_count())
                    .map(|sub_index| StreamOffset::new(item_index, sub_index as u32))
                    .collect::<Vec<_>>();
                let resulting_offset = *offsets.last().expect("validated input contains an item");
                let payload_for_high_water = payload_for_entry.clone();
                records.push(DurableStreamOplogRecord::Items(
                    entity_parent_start_index,
                    StreamItemsRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        stream_id,
                        producer_fingerprint,
                        first_sequence,
                        nested_stream_ids,
                        newly_registered_stream_ids,
                        payload: payload_for_entry,
                        offsets,
                    },
                ));
                if let Some(session_key) = ingress_session_key {
                    let logical_item_count = payload_for_high_water.logical_item_count() as u64;
                    let resulting_offset = StreamOffset::new(
                        item_index,
                        u32::try_from(logical_item_count - 1)
                            .expect("validated stream batch length fits in u32"),
                    );
                    records.push(DurableStreamOplogRecord::Session(
                        entity_parent_start_index,
                        Box::new(StreamSessionRecord::InputHighWater(
                            StreamSessionInputHighWaterRecord {
                                format_version: DURABLE_STREAM_FORMAT_VERSION,
                                session_key,
                                stream_id,
                                epoch: 1,
                                first_sequence,
                                payload: payload_for_high_water,
                                high_water: InputStreamHighWater {
                                    highest_contiguous_sequence: first_sequence
                                        + logical_item_count
                                        - 1,
                                    resulting_offset,
                                    terminal: false,
                                },
                            },
                        )),
                    ));
                }
                if let Some(producer) = external_producer {
                    records.push(DurableStreamOplogRecord::Session(
                        entity_parent_start_index,
                        Box::new(StreamSessionRecord::ExternalProducerState(
                            StreamExternalProducerStateRecord {
                                format_version: DURABLE_STREAM_FORMAT_VERSION,
                                session_key: external_session_key,
                                stream_id,
                                producer_id: producer.id,
                                epoch: producer.epoch,
                                sequence: producer.sequence,
                                next_sequence: producer
                                    .sequence
                                    .checked_add(item_count)
                                    .expect("validated attached sequence does not overflow"),
                                resulting_offset,
                            },
                        )),
                    ));
                }
                records
            }))
            .await
            .map_err(StreamStoreError::from)?;
        self.commit(context).await?;

        let AppliedWriteBatch {
            events: item_events,
            newly_registered_stream_count,
        } = self
            .apply_committed_write_batch(&mut index, entries)
            .await?;
        let item_offsets = item_events.iter().map(|event| event.offset).collect();
        let publication = self.enqueue_events(Some(context), stream_id, item_events, false)?;
        self.record_registered_streams(newly_registered_stream_count);
        drop(index);
        self.wait_for_publication(Some(context), publication)
            .await?;
        crate::metrics::durable_stream::record_producer_operation("write", false);
        tracing::debug!(
            stream_id = %stream_id,
            first_sequence,
            logical_item_count = item_count,
            nested_streams = newly_registered_stream_count,
            replayed = false,
            "Durable stream item batch committed"
        );
        Ok(ProducerWriteOutcome {
            value: item_offsets,
            replayed: false,
        })
    }

    pub(super) async fn apply_committed_write_batch(
        &self,
        index: &mut ProducerStreamIndex,
        entries: Vec<(OplogIndex, OplogEntry)>,
    ) -> Result<AppliedWriteBatch, StreamStoreError> {
        let mut registrations = Vec::new();
        let mut registered_ids = Vec::new();
        let mut events = Vec::new();
        for (oplog_index, entry) in entries {
            match entry {
                OplogEntry::StreamRegistered {
                    entity_parent_start_index,
                    record,
                    ..
                } => {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(StreamStoreError::Oplog)?;
                    registrations.push((oplog_index, entity_parent_start_index, record));
                }
                OplogEntry::StreamItems {
                    entity_parent_start_index,
                    record,
                    ..
                } => {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(StreamStoreError::Oplog)?;
                    registered_ids.extend_from_slice(&record.newly_registered_stream_ids);
                    events.extend(index.apply_item_batch(
                        oplog_index,
                        entity_parent_start_index,
                        std::mem::take(&mut registrations),
                        record,
                        self.environment_id,
                        &self.producer,
                        self.producer_fingerprint,
                    )?);
                }
                OplogEntry::StreamEnd {
                    entity_parent_start_index,
                    record,
                    ..
                } => {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(StreamStoreError::Oplog)?;
                    events.push(index.apply_end(
                        oplog_index,
                        entity_parent_start_index,
                        record,
                        self.producer_fingerprint,
                    )?);
                }
                OplogEntry::StreamSession {
                    entity_parent_start_index,
                    record,
                    ..
                } => {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(StreamStoreError::Oplog)?;
                    index.apply_session_references(entity_parent_start_index, &record)?;
                    match record {
                        StreamSessionRecord::ExternalProducerState(record) => {
                            index.apply_external_producer_state(&record);
                        }
                        StreamSessionRecord::InputHighWater(_) => {}
                        _ => unreachable!("write batch contains an unrelated session record"),
                    }
                }
                _ => unreachable!("write batch contains an unrelated oplog entry"),
            }
        }
        assert!(
            registrations.is_empty(),
            "write batch omitted enclosing item"
        );
        let count = registered_ids.len();
        let mut buses = self
            .buses
            .write()
            .expect("durable stream bus map lock poisoned");
        for stream_id in registered_ids {
            buses.insert(
                stream_id,
                Arc::new(DurableLiveStreamBus::new(self.live_join_capacity)?),
            );
        }
        Ok(AppliedWriteBatch {
            events,
            newly_registered_stream_count: count,
        })
    }
}
