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

use super::index::validate_items_payload;
use super::items::AppliedWriteBatch;
use super::*;

impl DurableStreamStore {
    /// Admits one externally sequenced input and commits it before reporting acceptance.
    pub async fn append_external_input(
        self: &Arc<Self>,
        context: Option<&StreamWriteContext>,
        session_key: &StreamSessionKey,
        stream_id: StreamId,
        payload: Option<StreamItemsPayload>,
        close: bool,
        producer: Option<ExternalProducer>,
    ) -> Result<ExternalAppendOutcome, StreamStoreError> {
        Self::validate_external_input(payload.as_ref())?;
        let retained_bytes = Self::external_input_reservation(payload.as_ref(), producer.as_ref());
        let session_key = session_key.clone();
        self.run_owned(context, retained_bytes, move |owner, context| async move {
            owner
                .append_external_input_owned(
                    &context,
                    &session_key,
                    stream_id,
                    payload,
                    close,
                    producer,
                )
                .await
        })
        .await
    }

    /// Rejects empty value batches, oversized items and malformed packed byte batches.
    pub(crate) fn validate_external_input(
        payload: Option<&StreamItemsPayload>,
    ) -> Result<(), StreamStoreError> {
        match payload {
            Some(StreamItemsPayload::Values(values)) => {
                if values.is_empty() {
                    return Err(StreamStoreError::InvalidValueBatch);
                }
                if values
                    .iter()
                    .any(|value| value.len() > MAX_DURABLE_STREAM_ITEM_SIZE)
                {
                    return Err(StreamStoreError::ItemTooLarge);
                }
                Ok(())
            }
            Some(payload @ StreamItemsPayload::PackedU8(_)) => validate_items_payload(payload),
            None => Ok(()),
        }
    }

    /// Bytes an admission has to reserve while the batch is queued and committed.
    pub(crate) fn external_input_reservation(
        payload: Option<&StreamItemsPayload>,
        producer: Option<&ExternalProducer>,
    ) -> usize {
        let payload_bytes = match payload {
            Some(StreamItemsPayload::Values(values)) => {
                values.iter().map(Vec::len).sum::<usize>() * 2
            }
            Some(StreamItemsPayload::PackedU8(bytes)) => {
                bytes.len() * (std::mem::size_of::<CommittedProducerStreamEvent>() + 2)
            }
            None => 0,
        };
        payload_bytes
            + producer.map_or(0, |producer| match &producer.id {
                ExternalProducerId::Client(id) => id.len(),
                ExternalProducerId::Attached => 0,
            })
    }

    /// Commits one externally sequenced input under an admission the caller already holds.
    /// The caller validated the batch, reserved [`Self::external_input_reservation`] bytes for
    /// it and keeps the session lock, so slot state it checked before submitting stays valid
    /// until the commit.
    pub(crate) async fn append_external_input_admitted(
        self: &Arc<Self>,
        admission: &Arc<StreamWriteAdmission>,
        session_key: &StreamSessionKey,
        stream_id: StreamId,
        payload: Option<StreamItemsPayload>,
        close: bool,
        producer: Option<ExternalProducer>,
    ) -> Result<ExternalAppendOutcome, StreamStoreError> {
        let session_key = session_key.clone();
        admission
            .submit(move |owner, context| async move {
                owner
                    .append_external_input_owned(
                        &context,
                        &session_key,
                        stream_id,
                        payload,
                        close,
                        producer,
                    )
                    .await
            })
            .await
    }

    async fn append_external_input_owned(
        &self,
        context: &StreamWriteContext,
        session_key: &StreamSessionKey,
        stream_id: StreamId,
        payload: Option<StreamItemsPayload>,
        close: bool,
        producer: Option<ExternalProducer>,
    ) -> Result<ExternalAppendOutcome, StreamStoreError> {
        let mut keys = vec![ProducerMetadataKey::Stream(stream_id)];
        if let Some(producer) = &producer {
            if matches!(&producer.id, ExternalProducerId::Client(id) if id.is_empty()) {
                return Err(StreamStoreError::InvalidValueBatch);
            }
            keys.push(ProducerMetadataKey::ExternalProducerHead(
                session_key.clone(),
                stream_id,
                producer.id.clone(),
            ));
            keys.push(ProducerMetadataKey::ExternalProducerSequence(
                session_key.clone(),
                stream_id,
                producer.id.clone(),
                producer.epoch,
                producer.sequence,
            ));
        }
        let mut index = self.index_for_terminal(keys, stream_id).await?;
        let Some(registration) = index.registrations.get(&stream_id) else {
            return Ok(ExternalAppendOutcome::NotFound);
        };
        if index.stream_sessions.get(&stream_id) != Some(session_key) {
            return Ok(ExternalAppendOutcome::NotFound);
        }
        if registration.source_kind != StreamSourceKind::ExternalInlineInput
            && !(registration.source_kind == StreamSourceKind::Nested
                && producer
                    .as_ref()
                    .is_some_and(|producer| producer.id == ExternalProducerId::Attached))
        {
            return Err(StreamStoreError::InvalidHandle);
        }
        let stream = &index.streams[&stream_id];
        index.ensure_producer_write_allowed()?;
        if payload.is_none() && !close {
            return Err(StreamStoreError::InvalidValueBatch);
        }
        if stream.terminal {
            if close
                && let Some(event) = &stream.terminal_event
                && matches!(
                    event.payload,
                    CommittedProducerStreamEventPayload::End(StreamEndResult::Ok)
                )
            {
                if payload.is_none() && producer.is_none() {
                    return Ok(ExternalAppendOutcome::Duplicate {
                        offset: event.offset,
                        highest_sequence: None,
                    });
                }
                if let Some(request) = &producer {
                    let identity = (session_key.clone(), stream_id, request.id.clone());
                    if let Some(head) = index.external_producer_heads.get(&identity)
                        && head.epoch == request.epoch
                        && head.last_sequence == request.sequence
                        && head.last_offset == event.offset
                    {
                        return Ok(ExternalAppendOutcome::Duplicate {
                            offset: event.offset,
                            highest_sequence: Some(head.last_sequence),
                        });
                    }
                }
            }
            return Ok(ExternalAppendOutcome::Closed);
        }
        if index.finished_sessions.contains(session_key) {
            return Ok(ExternalAppendOutcome::Closed);
        }
        if let Some(request) = &producer {
            let identity = (session_key.clone(), stream_id, request.id.clone());
            if let Some(head) = index.external_producer_heads.get(&identity) {
                if request.epoch < head.epoch {
                    return Ok(ExternalAppendOutcome::EpochFenced(head.epoch));
                }
                if request.epoch == head.epoch && request.sequence <= head.last_sequence {
                    if request.id == ExternalProducerId::Attached {
                        // Attached item retries validate their original payload through
                        // write_items_owned; an end frame cannot stand in for an item.
                        return Err(StreamStoreError::EventConflict);
                    }
                    let offset = index
                        .external_producer_offsets
                        .get(&(
                            identity.0,
                            identity.1,
                            identity.2,
                            request.epoch,
                            request.sequence,
                        ))
                        .copied()
                        .ok_or_else(|| {
                            StreamStoreError::CorruptHistory(
                                "external producer sequence has no original offset".into(),
                            )
                        })?;
                    return Ok(ExternalAppendOutcome::Duplicate {
                        offset,
                        highest_sequence: Some(head.last_sequence),
                    });
                }
                let expected = if request.epoch == head.epoch {
                    head.next_sequence
                } else {
                    0
                };
                if request.sequence != expected {
                    return Ok(ExternalAppendOutcome::SeqGap {
                        expected,
                        received: request.sequence,
                    });
                }
            } else if request.sequence != 0 {
                return Ok(ExternalAppendOutcome::SeqGap {
                    expected: 0,
                    received: request.sequence,
                });
            }
        }

        let first_sequence = stream.next_sequence;
        let item_count = payload
            .as_ref()
            .map_or(0, |value| value.logical_item_count()) as u64;
        let terminal_sequence = first_sequence
            .checked_add(item_count)
            .ok_or(StreamStoreError::CounterOverflow)?;
        let producer_fingerprint = self.producer_fingerprint;
        let session = session_key.clone();
        if let Some(producer) = &producer {
            producer
                .sequence
                .checked_add(1)
                .ok_or(StreamStoreError::CounterOverflow)?;
        }
        let producer_record = producer.clone();
        let entity_parent_start_index = index.entity_parent_start_index(stream_id)?;
        context.begin_durable_effect();
        let entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |first_index| {
                let mut records = Vec::new();
                let mut resulting_offset = None;
                if let Some(payload) = &payload {
                    match payload {
                        StreamItemsPayload::Values(values) => {
                            for (position, value) in values.iter().enumerate() {
                                let item_index =
                                    OplogIndex::from_u64(first_index.as_u64() + position as u64);
                                let offset = StreamOffset::new(item_index, 0);
                                resulting_offset = Some(offset);
                                records.push(DurableStreamOplogRecord::Items(
                                    entity_parent_start_index,
                                    StreamItemsRecord {
                                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                                        stream_id,
                                        producer_fingerprint,
                                        first_sequence: first_sequence + position as u64,
                                        nested_stream_ids: Vec::new(),
                                        newly_registered_stream_ids: Vec::new(),
                                        payload: StreamItemsPayload::Values(vec![value.clone()]),
                                        offsets: vec![offset],
                                    },
                                ));
                            }
                        }
                        StreamItemsPayload::PackedU8(_) => {
                            let offsets = (0..payload.logical_item_count())
                                .map(|sub| StreamOffset::new(first_index, sub as u32))
                                .collect::<Vec<_>>();
                            resulting_offset = offsets.last().copied();
                            records.push(DurableStreamOplogRecord::Items(
                                entity_parent_start_index,
                                StreamItemsRecord {
                                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                                    stream_id,
                                    producer_fingerprint,
                                    first_sequence,
                                    nested_stream_ids: Vec::new(),
                                    newly_registered_stream_ids: Vec::new(),
                                    payload: payload.clone(),
                                    offsets,
                                },
                            ));
                        }
                    }
                }
                let end_index =
                    OplogIndex::from_u64(first_index.as_u64() + records.len() as u64 + 1);
                if close {
                    resulting_offset = Some(StreamOffset::new(end_index, 0));
                }
                let resulting_offset =
                    resulting_offset.expect("external append has payload or terminal");
                records.push(DurableStreamOplogRecord::Session(
                    entity_parent_start_index,
                    Box::new(StreamSessionRecord::InputHighWater(
                        StreamSessionInputHighWaterRecord {
                            format_version: DURABLE_STREAM_FORMAT_VERSION,
                            session_key: session.clone(),
                            stream_id,
                            epoch: 1,
                            first_sequence,
                            payload: payload
                                .unwrap_or_else(|| StreamItemsPayload::Values(Vec::new())),
                            high_water: InputStreamHighWater {
                                highest_contiguous_sequence: if close {
                                    terminal_sequence
                                } else {
                                    terminal_sequence - 1
                                },
                                resulting_offset,
                                terminal: close,
                            },
                        },
                    )),
                ));
                if close {
                    records.push(DurableStreamOplogRecord::End(
                        entity_parent_start_index,
                        StreamEndRecord {
                            format_version: DURABLE_STREAM_FORMAT_VERSION,
                            stream_id,
                            producer_fingerprint,
                            sequence: terminal_sequence,
                            offset: resulting_offset,
                            authored_by: StreamTerminalAuthor::Protocol,
                            result: StreamEndResult::Ok,
                        },
                    ));
                }
                if let Some(producer) = producer_record {
                    records.push(DurableStreamOplogRecord::Session(
                        entity_parent_start_index,
                        Box::new(StreamSessionRecord::ExternalProducerState(
                            StreamExternalProducerStateRecord {
                                format_version: DURABLE_STREAM_FORMAT_VERSION,
                                session_key: session,
                                stream_id,
                                producer_id: producer.id,
                                epoch: producer.epoch,
                                sequence: producer.sequence,
                                next_sequence: producer.sequence.checked_add(1).expect(
                                    "validated external producer sequence does not overflow",
                                ),
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

        let AppliedWriteBatch { events, .. } = self
            .apply_committed_write_batch(&mut index, entries)
            .await?;
        let offset = events
            .last()
            .expect("external append committed no result offset")
            .offset;
        let publication = self.enqueue_events(Some(context), stream_id, events, false)?;
        if close {
            self.record_terminal_streams(1);
        }
        drop(index);
        self.wait_for_publication(Some(context), publication)
            .await?;
        Ok(ExternalAppendOutcome::Accepted(offset))
    }
}
