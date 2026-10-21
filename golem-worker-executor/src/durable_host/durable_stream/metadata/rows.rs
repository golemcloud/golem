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

impl ProducerStreamIndex {
    pub(in crate::durable_host::durable_stream) fn metadata_row(
        &self,
        key: &ProducerMetadataKey,
    ) -> Option<ProducerMetadataRow> {
        Some(match key {
            ProducerMetadataKey::Lineage => ProducerMetadataRow::Lineage(self.fork_lineage.clone()),
            ProducerMetadataKey::Global => ProducerMetadataRow::Global {
                open_streams: self.open_streams,
                active_attachments: self.active_attachment_count,
                session_count: self.session_count,
                deleting: self.deleting,
                consumer_deleting: self.consumer_deleting,
            },
            ProducerMetadataKey::Stream(id) => {
                let stream = self.streams.get(id)?;
                ProducerMetadataRow::Stream(Box::new(ProducerStreamMetadata {
                    registration: self.registrations.get(id)?.clone(),
                    session_key: self.stream_sessions.get(id)?.clone(),
                    role: *self.stream_roles.get(id)?,
                    entity_parent_start_index: self
                        .entity_parent_start_indices
                        .get(id)
                        .copied()
                        .flatten(),
                    first_sequence: stream.first_sequence,
                    next_sequence: stream.next_sequence,
                    last_offset: stream.last_offset,
                    last_item_offset: stream.last_item_offset,
                    terminal: stream.terminal,
                }))
            }
            ProducerMetadataKey::Coordinate(coordinate) => {
                ProducerMetadataRow::Coordinate(*self.coordinates.get(coordinate)?)
            }
            ProducerMetadataKey::SessionPage(page) => {
                ProducerMetadataRow::SessionPage(self.session_pages.get(page)?.clone())
            }
            ProducerMetadataKey::Session(key) => {
                ProducerMetadataRow::Session(ProducerSessionMetadata {
                    consumer_streams: self
                        .session_consumer_streams
                        .get(key)
                        .cloned()
                        .unwrap_or_default(),
                    mappings: self
                        .session_stream_mappings
                        .get(key)
                        .cloned()
                        .unwrap_or_default(),
                    open_streams: self
                        .open_session_streams
                        .get(key)
                        .cloned()
                        .unwrap_or_default(),
                    entity_parent_start_index: *self
                        .session_entity_parent_start_indices
                        .get(key)?,
                    invocation_result: self.invocation_results.get(key).copied(),
                    finished: self.finished_sessions.contains(key),
                })
            }
            ProducerMetadataKey::Batch(id, sequence) => {
                ProducerMetadataRow::Batch(*self.streams.get(id)?.batches.get(sequence)?)
            }
            ProducerMetadataKey::Position(stream, offset) => {
                let (first, count) = self.batch_positions.get(&(*stream, *offset))?;
                ProducerMetadataRow::Position(*first, *count)
            }
            ProducerMetadataKey::Attachment(attachment, stream, environment, consumer) => {
                ProducerMetadataRow::Attachment(Box::new(
                    self.attachments
                        .get(&(*attachment, *stream, *environment, consumer.clone()))?
                        .clone(),
                ))
            }
            ProducerMetadataKey::ActiveAttachmentCount(session, stream) => {
                ProducerMetadataRow::ActiveAttachmentCount(
                    self.active_attachments_by_session_stream
                        .get(&(session.clone(), *stream))
                        .copied()
                        .unwrap_or_default(),
                )
            }
            ProducerMetadataKey::ConsumerHead(session, stream) => {
                ProducerMetadataRow::ConsumerHead(
                    self.consumer_journals
                        .get(&(session.clone(), *stream))?
                        .clone(),
                )
            }
            ProducerMetadataKey::Cascade(key) => {
                ProducerMetadataRow::Cascade(self.cascade_outbox.get(key.as_ref())?.clone())
            }
            ProducerMetadataKey::AttachmentPage(page) => {
                ProducerMetadataRow::AttachmentPage(self.attachment_pages.get(page)?.clone())
            }
            ProducerMetadataKey::AttachmentPosition(attachment, stream, environment, consumer) => {
                ProducerMetadataRow::AttachmentPosition(
                    self.attachment_positions
                        .get(&(*attachment, *stream, *environment, consumer.clone()))
                        .copied()
                        .flatten(),
                )
            }
            ProducerMetadataKey::ExternalProducerHead(session, stream, producer) => {
                ProducerMetadataRow::ExternalProducer(
                    self.external_producer_heads
                        .get(&(session.clone(), *stream, producer.clone()))?
                        .clone(),
                )
            }
            ProducerMetadataKey::ExternalProducerSequence(
                session,
                stream,
                producer,
                epoch,
                sequence,
            ) => ProducerMetadataRow::ExternalProducerOffset(*self.external_producer_offsets.get(
                &(
                    session.clone(),
                    *stream,
                    producer.clone(),
                    *epoch,
                    *sequence,
                ),
            )?),
        })
    }

    pub(in crate::durable_host::durable_stream) fn hydrate_metadata_row(
        &mut self,
        key: ProducerMetadataKey,
        row: ProducerMetadataRow,
    ) -> Result<(), String> {
        match (key, row) {
            (_, ProducerMetadataRow::Retired) => {}
            (ProducerMetadataKey::Lineage, ProducerMetadataRow::Lineage(lineage)) => {
                self.fork_lineage = lineage;
            }
            (
                ProducerMetadataKey::Global,
                ProducerMetadataRow::Global {
                    open_streams,
                    active_attachments,
                    session_count,
                    deleting,
                    consumer_deleting,
                },
            ) => {
                self.open_streams = open_streams;
                self.active_attachment_count = active_attachments;
                self.session_count = session_count;
                self.deleting = deleting;
                self.consumer_deleting = consumer_deleting;
            }
            (ProducerMetadataKey::Stream(id), ProducerMetadataRow::Stream(value)) => {
                if id != value.registration.handle.stream_id {
                    return Err("producer metadata stream identity mismatch".into());
                }
                self.local_stream_ids.insert(
                    LocalStreamId(value.registration.registration_oplog_index),
                    id,
                );
                self.coordinates
                    .insert(value.registration.coordinate.clone(), id);
                self.registrations.insert(id, value.registration);
                self.stream_sessions.insert(id, value.session_key);
                self.stream_roles.insert(id, value.role);
                self.entity_parent_start_indices
                    .insert(id, value.entity_parent_start_index);
                self.streams.insert(
                    id,
                    IndexedProducerStream {
                        first_sequence: value.first_sequence,
                        next_sequence: value.next_sequence,
                        last_offset: value.last_offset,
                        last_item_offset: value.last_item_offset,
                        terminal: value.terminal,
                        ..Default::default()
                    },
                );
            }
            (
                ProducerMetadataKey::Coordinate(coordinate),
                ProducerMetadataRow::Coordinate(stream),
            ) => {
                self.coordinates.insert(coordinate, stream);
            }
            (
                ProducerMetadataKey::SessionPage(page),
                ProducerMetadataRow::SessionPage(sessions),
            ) => {
                self.session_pages.insert(page, sessions);
            }
            (ProducerMetadataKey::Session(key), ProducerMetadataRow::Session(value)) => {
                self.session_stream_counts
                    .insert(key.clone(), value.mappings.len());
                self.session_stream_mappings
                    .insert(key.clone(), value.mappings);
                self.open_session_streams
                    .insert(key.clone(), value.open_streams);
                self.session_consumer_streams
                    .insert(key.clone(), value.consumer_streams);
                self.session_entity_parent_start_indices
                    .insert(key.clone(), value.entity_parent_start_index);
                if let Some(offset) = value.invocation_result {
                    self.invocation_results.insert(key.clone(), offset);
                }
                if value.finished {
                    self.finished_sessions.insert(key);
                }
            }
            (ProducerMetadataKey::Batch(stream, sequence), ProducerMetadataRow::Batch(offset)) => {
                self.streams
                    .get_mut(&stream)
                    .ok_or("batch metadata requires its stream summary")?
                    .batches
                    .insert(sequence, offset);
            }
            (
                ProducerMetadataKey::Attachment(attachment, stream, environment, consumer),
                ProducerMetadataRow::Attachment(value),
            ) => {
                if value.key.attachment_id != attachment
                    || value.key.stream_id != stream
                    || value.key.consumer_environment_id != environment
                    || value.key.consumer != consumer
                {
                    return Err("producer metadata attachment identity mismatch".into());
                }
                self.attachments
                    .insert((attachment, stream, environment, consumer), *value);
            }
            (
                ProducerMetadataKey::ConsumerHead(session, stream),
                ProducerMetadataRow::ConsumerHead(value),
            ) => {
                self.consumer_journals.insert((session, stream), value);
            }
            (
                ProducerMetadataKey::ActiveAttachmentCount(session, stream),
                ProducerMetadataRow::ActiveAttachmentCount(value),
            ) => {
                self.active_attachments_by_session_stream
                    .insert((session, stream), value);
            }
            (ProducerMetadataKey::Cascade(key), ProducerMetadataRow::Cascade(value)) => {
                self.cascade_outbox.insert(*key, value);
            }
            (
                ProducerMetadataKey::AttachmentPage(page),
                ProducerMetadataRow::AttachmentPage(value),
            ) => {
                self.attachment_pages.insert(page, value);
            }
            (
                ProducerMetadataKey::AttachmentPosition(attachment, stream, environment, consumer),
                ProducerMetadataRow::AttachmentPosition(value),
            ) => {
                self.attachment_positions
                    .insert((attachment, stream, environment, consumer), value);
            }
            (
                ProducerMetadataKey::Position(stream, offset),
                ProducerMetadataRow::Position(first, count),
            ) => {
                self.batch_positions
                    .insert((stream, offset), (first, count));
            }
            (
                ProducerMetadataKey::ExternalProducerHead(session, stream, producer),
                ProducerMetadataRow::ExternalProducer(value),
            ) => {
                self.external_producer_heads
                    .insert((session, stream, producer), value);
            }
            (
                ProducerMetadataKey::ExternalProducerSequence(
                    session,
                    stream,
                    producer,
                    epoch,
                    sequence,
                ),
                ProducerMetadataRow::ExternalProducerOffset(offset),
            ) => {
                self.external_producer_offsets
                    .insert((session, stream, producer, epoch, sequence), offset);
            }
            _ => return Err("producer metadata row type does not match its key".into()),
        }
        Ok(())
    }
}
