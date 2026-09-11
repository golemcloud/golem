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
use crate::storage::keyvalue::{
    KeyValueStorage, KeyValueStorageLabelledApi, KeyValueStorageNamespace,
};
use golem_common::serialization::serialize;

const ATTACHMENT_PAGE_SIZE: u64 = 128;

#[derive(Clone, Debug, Eq, Hash, PartialEq, desert_rust::BinaryCodec)]
pub enum ProducerMetadataKey {
    Global,
    Stream(StreamId),
    Coordinate(StreamRegistrationCoordinateV1),
    Reference(StreamId),
    Session(StreamSessionKeyV1),
    Batch(StreamId, u64),
    Attachment(AttachmentId, StreamId),
    ConsumerHead(StreamSessionKeyV1, StreamId),
    Cascade(Box<StreamAttachmentKeyV1>),
    AttachmentPage(u64),
    AttachmentPosition(AttachmentId, StreamId),
    Position(StreamId, OplogIndex),
    ExternalProducerHead(StreamSessionKeyV1, StreamId, String),
    ExternalProducerSequence(StreamSessionKeyV1, StreamId, String, u64, u64),
}

impl ProducerMetadataKey {
    pub(crate) fn field(&self) -> Result<String, String> {
        Ok(format!("producer:{}", hex::encode(serialize(self)?)))
    }

    pub(super) fn registration(request: &ProducerRegistrationRequestV1) -> Vec<Self> {
        let mut keys = vec![Self::Coordinate(request.coordinate.clone())];
        if let Some(mapping) = &request.session_mapping {
            keys.push(Self::Session(mapping.session_key.clone()));
        }
        match &request.coordinate {
            StreamRegistrationCoordinateV1::Root { invocation_id, .. } => {
                keys.push(Self::Session(invocation_id.clone()));
            }
            StreamRegistrationCoordinateV1::Nested {
                parent_stream_id, ..
            } => {
                keys.push(Self::Stream(*parent_stream_id));
            }
        }
        keys
    }

    pub(super) fn session_record(record: &StreamSessionRecordV1) -> Vec<Self> {
        let mut keys = Vec::new();
        if let Some(key) = crate::worker::stream_session_record_key(record) {
            keys.push(Self::Session(key.clone()));
        }
        let mappings = match record {
            StreamSessionRecordV1::Prepared(record) => record.stream_mappings.as_slice(),
            StreamSessionRecordV1::InvocationResult(record) => record.stream_mappings.as_slice(),
            StreamSessionRecordV1::Mapping(record) => std::slice::from_ref(&record.mapping),
            _ => &[],
        };
        keys.extend(
            mappings
                .iter()
                .map(|mapping| Self::Reference(mapping.handle.stream_id)),
        );
        let attachment = match record {
            StreamSessionRecordV1::AttachmentPrepared(record) => Some(&record.key),
            StreamSessionRecordV1::AttachmentActivated(record) => Some(&record.key),
            StreamSessionRecordV1::AttachmentRenewed(record) => Some(&record.key),
            StreamSessionRecordV1::AttachmentFinalized(record) => Some(&record.key),
            StreamSessionRecordV1::CascadeOutbox(record) => {
                keys.push(Self::Cascade(Box::new(record.key.clone())));
                Some(&record.key)
            }
            _ => None,
        };
        if let Some(key) = attachment {
            keys.push(Self::Attachment(key.attachment_id, key.stream_id));
        }
        let head = match record {
            StreamSessionRecordV1::ConsumerItemValue(record) => {
                Some((&record.session_key, record.stream_id))
            }
            StreamSessionRecordV1::ConsumerTerminal(record) => {
                Some((&record.session_key, record.stream_id))
            }
            StreamSessionRecordV1::SourceUnavailable(record) => {
                Some((&record.key.session_key, record.key.stream_id))
            }
            _ => None,
        };
        if let Some((session, stream)) = head {
            keys.push(Self::ConsumerHead(session.clone(), stream));
        }
        if let StreamSessionRecordV1::ExternalProducerState(record) = record {
            keys.push(Self::ExternalProducerHead(
                record.session_key.clone(),
                record.stream_id,
                record.producer_id.clone(),
            ));
            keys.push(Self::ExternalProducerSequence(
                record.session_key.clone(),
                record.stream_id,
                record.producer_id.clone(),
                record.epoch,
                record.sequence,
            ));
        }
        keys
    }
}

#[derive(Clone, desert_rust::BinaryCodec)]
pub struct ProducerStreamMetadata {
    registration: StreamRegisteredRecordV1,
    session_key: StreamSessionKeyV1,
    role: SessionStreamRoleV1,
    first_sequence: Option<u64>,
    next_sequence: u64,
    last_offset: Option<StreamOffsetV1>,
    terminal: bool,
}

#[derive(Clone, desert_rust::BinaryCodec)]
pub struct ProducerSessionMetadata {
    mappings: HashSet<(DurableStreamHandleV1, SessionStreamRoleV1)>,
    references: HashSet<StreamId>,
    open_streams: HashSet<StreamId>,
    invocation_result: Option<OplogIndex>,
    finished: bool,
}

#[derive(Clone, desert_rust::BinaryCodec)]
pub enum ProducerMetadataRow {
    Global {
        open_streams: usize,
        active_attachments: u64,
        deleting: bool,
        consumer_deleting: bool,
    },
    Stream(Box<ProducerStreamMetadata>),
    Coordinate(StreamId),
    Reference(DurableStreamHandleV1),
    Session(ProducerSessionMetadata),
    Batch(OplogIndex),
    Attachment(IndexedStreamAttachment),
    ConsumerHead(IndexedConsumerJournal),
    Cascade(StreamCascadeDependentResultV1),
    AttachmentPage(Vec<(AttachmentId, StreamId)>),
    AttachmentPosition(Option<u64>),
    Position(u64, u64),
    ExternalProducer(IndexedExternalProducer),
    ExternalProducerOffset(StreamOffsetV1),
}

impl ProducerStreamIndex {
    pub(super) fn metadata_row(&self, key: &ProducerMetadataKey) -> Option<ProducerMetadataRow> {
        Some(match key {
            ProducerMetadataKey::Global => ProducerMetadataRow::Global {
                open_streams: self.open_streams,
                active_attachments: self.active_attachment_count,
                deleting: self.deleting,
                consumer_deleting: self.consumer_deleting,
            },
            ProducerMetadataKey::Stream(id) => {
                let stream = self.streams.get(id)?;
                ProducerMetadataRow::Stream(Box::new(ProducerStreamMetadata {
                    registration: self.registrations.get(id)?.clone(),
                    session_key: self.stream_sessions.get(id)?.clone(),
                    role: *self.stream_roles.get(id)?,
                    first_sequence: stream.first_sequence,
                    next_sequence: stream.next_sequence,
                    last_offset: stream.last_offset,
                    terminal: stream.terminal,
                }))
            }
            ProducerMetadataKey::Coordinate(coordinate) => {
                ProducerMetadataRow::Coordinate(*self.coordinates.get(coordinate)?)
            }
            ProducerMetadataKey::Reference(id) => {
                ProducerMetadataRow::Reference(self.referenced_handles.get(id)?.0.clone())
            }
            ProducerMetadataKey::Session(key) => {
                ProducerMetadataRow::Session(ProducerSessionMetadata {
                    mappings: self
                        .session_stream_mappings
                        .get(key)
                        .cloned()
                        .unwrap_or_default(),
                    references: self
                        .referenced_handles
                        .iter()
                        .filter_map(|(id, (_, sessions))| sessions.contains(key).then_some(*id))
                        .collect(),
                    open_streams: self
                        .open_session_streams
                        .get(key)
                        .cloned()
                        .unwrap_or_default(),
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
            ProducerMetadataKey::Attachment(attachment, stream) => ProducerMetadataRow::Attachment(
                self.attachments.get(&(*attachment, *stream))?.clone(),
            ),
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
            ProducerMetadataKey::AttachmentPosition(attachment, stream) => {
                ProducerMetadataRow::AttachmentPosition(
                    self.attachment_positions
                        .get(&(*attachment, *stream))
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

    pub(super) fn hydrate_metadata_row(
        &mut self,
        key: ProducerMetadataKey,
        row: ProducerMetadataRow,
    ) -> Result<(), String> {
        match (key, row) {
            (
                ProducerMetadataKey::Global,
                ProducerMetadataRow::Global {
                    open_streams,
                    active_attachments,
                    deleting,
                    consumer_deleting,
                },
            ) => {
                self.open_streams = open_streams;
                self.active_attachment_count = active_attachments;
                self.deleting = deleting;
                self.consumer_deleting = consumer_deleting;
            }
            (ProducerMetadataKey::Stream(id), ProducerMetadataRow::Stream(value)) => {
                if id != value.registration.handle.stream_id {
                    return Err("producer metadata stream identity mismatch".into());
                }
                self.coordinates
                    .insert(value.registration.coordinate.clone(), id);
                self.registrations.insert(id, value.registration);
                self.stream_sessions.insert(id, value.session_key);
                self.stream_roles.insert(id, value.role);
                self.streams.insert(
                    id,
                    IndexedProducerStream {
                        first_sequence: value.first_sequence,
                        next_sequence: value.next_sequence,
                        last_offset: value.last_offset,
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
            (ProducerMetadataKey::Reference(stream), ProducerMetadataRow::Reference(handle)) => {
                self.referenced_handles
                    .entry(stream)
                    .or_insert_with(|| (handle, HashSet::new()));
            }
            (ProducerMetadataKey::Session(key), ProducerMetadataRow::Session(value)) => {
                for (handle, _) in &value.mappings {
                    if value.references.contains(&handle.stream_id) {
                        self.referenced_handles
                            .entry(handle.stream_id)
                            .or_insert_with(|| (handle.clone(), HashSet::new()))
                            .1
                            .insert(key.clone());
                    }
                }
                self.session_stream_counts
                    .insert(key.clone(), value.mappings.len());
                self.session_stream_mappings
                    .insert(key.clone(), value.mappings);
                self.open_session_streams
                    .insert(key.clone(), value.open_streams);
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
                ProducerMetadataKey::Attachment(attachment, stream),
                ProducerMetadataRow::Attachment(value),
            ) => {
                self.attachments.insert((attachment, stream), value);
            }
            (
                ProducerMetadataKey::ConsumerHead(session, stream),
                ProducerMetadataRow::ConsumerHead(value),
            ) => {
                self.consumer_journals.insert((session, stream), value);
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
                ProducerMetadataKey::AttachmentPosition(attachment, stream),
                ProducerMetadataRow::AttachmentPosition(value),
            ) => {
                self.attachment_positions
                    .insert((attachment, stream), value);
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

struct Projection<'a> {
    index: ProducerStreamIndex,
    loaded: HashSet<ProducerMetadataKey>,
    storage: &'a (dyn KeyValueStorage + Send + Sync),
    namespace: KeyValueStorageNamespace,
}

impl Projection<'_> {
    async fn load(&mut self, key: ProducerMetadataKey) -> Result<(), String> {
        if self.loaded.insert(key.clone()) {
            let row = self
                .storage
                .with_entity("worker", "producer_projection_read", "metadata")
                .get(self.namespace.clone(), &key.field()?)
                .await?;
            if let Some(row) = row {
                self.index.hydrate_metadata_row(key, row)?;
            }
        }
        Ok(())
    }

    async fn catalogue_attachment(
        &mut self,
        attachment: AttachmentId,
        stream: StreamId,
    ) -> Result<(), String> {
        let key = (attachment, stream);
        self.load(ProducerMetadataKey::AttachmentPosition(attachment, stream))
            .await?;
        let position = self.index.attachment_positions.get(&key).copied().flatten();
        let active = !matches!(
            self.index.attachments[&key].state,
            IndexedStreamAttachmentState::Finalized { .. }
        );
        match (active, position) {
            (true, None) => {
                let position = self.index.active_attachment_count;
                let page = position / ATTACHMENT_PAGE_SIZE;
                self.load(ProducerMetadataKey::AttachmentPage(page)).await?;
                self.index
                    .attachment_pages
                    .entry(page)
                    .or_default()
                    .push(key);
                self.index.attachment_positions.insert(key, Some(position));
                self.index.active_attachment_count += 1;
            }
            (false, Some(position)) => {
                let last = self
                    .index
                    .active_attachment_count
                    .checked_sub(1)
                    .ok_or("active attachment catalogue is empty")?;
                if position > last {
                    return Err("active attachment position exceeds its catalogue".into());
                }
                self.load(ProducerMetadataKey::AttachmentPage(
                    position / ATTACHMENT_PAGE_SIZE,
                ))
                .await?;
                self.load(ProducerMetadataKey::AttachmentPage(
                    last / ATTACHMENT_PAGE_SIZE,
                ))
                .await?;
                let moved = self
                    .index
                    .attachment_pages
                    .get_mut(&(last / ATTACHMENT_PAGE_SIZE))
                    .and_then(Vec::pop)
                    .ok_or("active attachment catalogue page is missing")?;
                if position != last {
                    self.load(ProducerMetadataKey::AttachmentPosition(moved.0, moved.1))
                        .await?;
                    *self
                        .index
                        .attachment_pages
                        .get_mut(&(position / ATTACHMENT_PAGE_SIZE))
                        .and_then(|page| page.get_mut((position % ATTACHMENT_PAGE_SIZE) as usize))
                        .ok_or("active attachment catalogue slot is missing")? = moved;
                    self.index
                        .attachment_positions
                        .insert(moved, Some(position));
                }
                self.index.attachment_positions.insert(key, None);
                self.index.active_attachment_count = last;
            }
            _ => {}
        }
        Ok(())
    }

    async fn stream(&mut self, stream: StreamId) -> Result<(), String> {
        self.load(ProducerMetadataKey::Stream(stream)).await?;
        if let Some(session) = self.index.stream_sessions.get(&stream).cloned() {
            self.load(ProducerMetadataKey::Session(session)).await?;
        }
        Ok(())
    }

    async fn registration(&mut self, record: &StreamRegisteredRecordV1) -> Result<(), String> {
        if let StreamRegistrationCoordinateV1::Nested {
            parent_stream_id, ..
        } = &record.coordinate
        {
            self.stream(*parent_stream_id).await?;
        }
        self.load(ProducerMetadataKey::Coordinate(record.coordinate.clone()))
            .await?;
        if let Some(existing) = self.index.coordinates.get(&record.coordinate).copied() {
            self.stream(existing).await?;
        }
        self.stream(record.handle.stream_id).await?;
        if let Some(session) = self
            .index
            .registration_session_key(&record.coordinate, &record.session_mapping)
        {
            self.load(ProducerMetadataKey::Session(session)).await?;
        }
        Ok(())
    }

    async fn session(&mut self, record: &StreamSessionRecordV1) -> Result<(), String> {
        if let Some(session) = crate::worker::stream_session_record_key(record) {
            self.load(ProducerMetadataKey::Session(session.clone()))
                .await?;
        }
        let mappings = match record {
            StreamSessionRecordV1::Prepared(record) => record.stream_mappings.as_slice(),
            StreamSessionRecordV1::InvocationResult(record) => record.stream_mappings.as_slice(),
            StreamSessionRecordV1::Mapping(record) => std::slice::from_ref(&record.mapping),
            _ => &[],
        };
        for mapping in mappings {
            self.load(ProducerMetadataKey::Reference(mapping.handle.stream_id))
                .await?;
        }
        let attachment = match record {
            StreamSessionRecordV1::AttachmentPrepared(record) => Some(&record.key),
            StreamSessionRecordV1::AttachmentActivated(record) => Some(&record.key),
            StreamSessionRecordV1::AttachmentRenewed(record) => Some(&record.key),
            StreamSessionRecordV1::AttachmentFinalized(record) => Some(&record.key),
            StreamSessionRecordV1::CascadeOutbox(record) => {
                self.load(ProducerMetadataKey::Cascade(Box::new(record.key.clone())))
                    .await?;
                Some(&record.key)
            }
            _ => None,
        };
        if let Some(key) = attachment {
            self.stream(key.stream_id).await?;
            self.load(ProducerMetadataKey::Attachment(
                key.attachment_id,
                key.stream_id,
            ))
            .await?;
        }
        let head = match record {
            StreamSessionRecordV1::ConsumerItemValue(record) => {
                Some((&record.session_key, record.stream_id))
            }
            StreamSessionRecordV1::ConsumerTerminal(record) => {
                Some((&record.session_key, record.stream_id))
            }
            StreamSessionRecordV1::SourceUnavailable(record) => {
                Some((&record.key.session_key, record.key.stream_id))
            }
            _ => None,
        };
        if let Some((session, stream)) = head {
            self.load(ProducerMetadataKey::ConsumerHead(session.clone(), stream))
                .await?;
        }
        if let StreamSessionRecordV1::ExternalProducerState(record) = record {
            self.load(ProducerMetadataKey::ExternalProducerHead(
                record.session_key.clone(),
                record.stream_id,
                record.producer_id.clone(),
            ))
            .await?;
            self.loaded
                .insert(ProducerMetadataKey::ExternalProducerSequence(
                    record.session_key.clone(),
                    record.stream_id,
                    record.producer_id.clone(),
                    record.epoch,
                    record.sequence,
                ));
        }
        Ok(())
    }
}

pub(crate) async fn project_producer_metadata(
    storage: &(dyn KeyValueStorage + Send + Sync),
    namespace: KeyValueStorageNamespace,
    oplog: &dyn OplogService,
    owner: &OwnedAgentId,
    mode: AgentMode,
    fingerprint: AgentFingerprint,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut projection = Projection {
        index: ProducerStreamIndex::default(),
        loaded: HashSet::new(),
        storage,
        namespace,
    };
    projection.load(ProducerMetadataKey::Global).await?;
    let mut pending = Vec::new();
    for (index, entry) in entries {
        if !pending.is_empty()
            && !matches!(
                entry,
                OplogEntry::StreamRegistered { .. } | OplogEntry::StreamItems { .. }
            )
        {
            return Err("nested registration batch is missing its enclosing item".into());
        }
        match entry {
            OplogEntry::StreamRegistered { record, .. } => {
                let record = oplog.download_payload(owner, mode, record.clone()).await?;
                projection.registration(&record).await?;
                if matches!(
                    record.coordinate,
                    StreamRegistrationCoordinateV1::Nested { .. }
                ) {
                    pending.push((*index, record));
                } else {
                    if !pending.is_empty() {
                        return Err(
                            "nested registration batch is missing its enclosing item".into()
                        );
                    }
                    projection
                        .index
                        .apply_registration(
                            *index,
                            record,
                            owner.environment_id,
                            &owner.agent_id,
                            fingerprint,
                        )
                        .map_err(|error| error.to_string())?;
                }
            }
            OplogEntry::StreamItems { record, .. } => {
                let record = oplog.download_payload(owner, mode, record.clone()).await?;
                projection.stream(record.stream_id).await?;
                for stream in &record.nested_stream_ids {
                    projection
                        .load(ProducerMetadataKey::Stream(*stream))
                        .await?;
                    projection
                        .load(ProducerMetadataKey::Reference(*stream))
                        .await?;
                }
                projection.loaded.insert(ProducerMetadataKey::Batch(
                    record.stream_id,
                    record.first_sequence,
                ));
                projection
                    .loaded
                    .insert(ProducerMetadataKey::Position(record.stream_id, *index));
                projection
                    .index
                    .apply_item_batch(
                        *index,
                        std::mem::take(&mut pending),
                        record,
                        owner.environment_id,
                        &owner.agent_id,
                        fingerprint,
                    )
                    .map_err(|error| error.to_string())?;
            }
            OplogEntry::StreamEnd { record, .. } => {
                let record = oplog.download_payload(owner, mode, record.clone()).await?;
                projection.stream(record.stream_id).await?;
                projection
                    .index
                    .apply_end(*index, record, fingerprint)
                    .map_err(|error| error.to_string())?;
            }
            OplogEntry::StreamCancel { record, .. } => {
                let record = oplog.download_payload(owner, mode, record.clone()).await?;
                projection.stream(record.stream_id).await?;
                projection
                    .index
                    .apply_cancel(*index, record, fingerprint)
                    .map_err(|error| error.to_string())?;
            }
            OplogEntry::StreamSession { record, .. } => {
                let record = oplog.download_payload(owner, mode, record.clone()).await?;
                projection.session(&record).await?;
                projection
                    .index
                    .apply_session_references(&record)
                    .map_err(|error| error.to_string())?;
                projection.index.apply_result_offset(*index, &record);
                if let StreamSessionRecordV1::ExternalProducerState(value) = &record {
                    projection.index.apply_external_producer_state(value);
                }
                projection
                    .index
                    .apply_deletion_record(
                        &record,
                        owner.environment_id,
                        &owner.agent_id,
                        fingerprint,
                    )
                    .map_err(|error| error.to_string())?;
                projection
                    .index
                    .apply_attachment_record(
                        &record,
                        owner.environment_id,
                        &owner.agent_id,
                        fingerprint,
                    )
                    .map_err(|error| error.to_string())?;
                for key in ProducerMetadataKey::session_record(&record) {
                    if let ProducerMetadataKey::Attachment(attachment, stream) = key {
                        projection.catalogue_attachment(attachment, stream).await?;
                    }
                }
                if let StreamSessionRecordV1::Finished(record) = &record {
                    projection
                        .index
                        .apply_finished(record)
                        .map_err(|error| error.to_string())?;
                }
            }
            _ => {}
        }
    }
    if !pending.is_empty() {
        return Err("producer projection chunk splits a nested registration batch".into());
    }
    projection
        .loaded
        .into_iter()
        .filter_map(|key| projection.index.metadata_row(&key).map(|row| (key, row)))
        .map(|(key, row)| Ok((key.field()?, serialize(&row)?)))
        .collect()
}

impl DurableStreamProducer {
    pub(crate) async fn load_indexed_with_commit(
        oplog: Arc<dyn Oplog>,
        owner: OwnedAgentId,
        producer_fingerprint: AgentFingerprint,
        live_join_capacity: Option<usize>,
        commit: DurableStreamCommit,
        service: Arc<dyn WorkerService>,
        mode: AgentMode,
    ) -> Result<Arc<Self>, DurableStreamProducerError> {
        let live_join_capacity = live_join_capacity.unwrap_or(DEFAULT_LIVE_JOIN_BUFFER_SIZE);
        DurableLiveStreamBus::<CommittedProducerStreamEventV1>::new(live_join_capacity)?;
        let (_, mut rows) = service
            .lookup_durable_stream_producer_metadata(
                &owner,
                mode,
                vec![ProducerMetadataKey::Global],
            )
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        if rows.len() != 1 {
            return Err(DurableStreamProducerError::CorruptHistory(
                "producer metadata lookup returned an incorrect row count".into(),
            ));
        }
        let mut index = ProducerStreamIndex::default();
        if let Some(row) = rows.pop().flatten() {
            index
                .hydrate_metadata_row(ProducerMetadataKey::Global, row)
                .map_err(DurableStreamProducerError::CorruptHistory)?;
        }
        index.loaded_metadata.insert(ProducerMetadataKey::Global);
        let producer = Self::from_index(
            oplog,
            owner.environment_id,
            owner.agent_id,
            producer_fingerprint,
            live_join_capacity,
            commit,
            index,
        )?;
        producer.set_control_metadata_provider(service, mode);
        Ok(producer)
    }

    pub(super) async fn index_for(
        &self,
        keys: impl IntoIterator<Item = ProducerMetadataKey> + Send,
    ) -> Result<MutexGuard<'_, ProducerStreamIndex>, DurableStreamProducerError> {
        self.index_for_query(keys.into_iter().collect(), None, None)
            .await
    }

    pub(super) async fn index_for_terminal(
        &self,
        keys: impl IntoIterator<Item = ProducerMetadataKey> + Send,
        stream: StreamId,
    ) -> Result<MutexGuard<'_, ProducerStreamIndex>, DurableStreamProducerError> {
        let mut keys = keys.into_iter().collect::<Vec<_>>();
        keys.push(ProducerMetadataKey::Stream(stream));
        self.index_for_query(keys, Some(stream), None).await
    }

    pub(super) async fn index_for_session_streams(
        &self,
        session: &StreamSessionKeyV1,
    ) -> Result<MutexGuard<'_, ProducerStreamIndex>, DurableStreamProducerError> {
        self.index_for_query(
            vec![ProducerMetadataKey::Session(session.clone())],
            None,
            Some(session.clone()),
        )
        .await
    }

    fn query_keys(
        &self,
        index: &ProducerStreamIndex,
        keys: &[ProducerMetadataKey],
        session: Option<&StreamSessionKeyV1>,
    ) -> HashSet<ProducerMetadataKey> {
        let mut pending = keys.to_vec();
        pending.push(ProducerMetadataKey::Global);
        if let Some(session) = session {
            pending.extend(
                index
                    .session_stream_mappings
                    .get(session)
                    .into_iter()
                    .flatten()
                    .filter(|(handle, _)| self.owns_handle_identity(handle))
                    .map(|(handle, _)| ProducerMetadataKey::Stream(handle.stream_id)),
            );
            pending.extend(
                index
                    .open_session_streams
                    .get(session)
                    .into_iter()
                    .flatten()
                    .map(|id| ProducerMetadataKey::Stream(*id)),
            );
        }
        let mut required = HashSet::new();
        while let Some(key) = pending.pop() {
            if !required.insert(key.clone()) {
                continue;
            }
            match key {
                ProducerMetadataKey::Batch(stream, _)
                | ProducerMetadataKey::Attachment(_, stream) => {
                    pending.push(ProducerMetadataKey::Stream(stream))
                }
                ProducerMetadataKey::Stream(stream) => pending.extend(
                    index
                        .stream_sessions
                        .get(&stream)
                        .cloned()
                        .map(ProducerMetadataKey::Session),
                ),
                ProducerMetadataKey::Coordinate(coordinate) => pending.extend(
                    index
                        .coordinates
                        .get(&coordinate)
                        .copied()
                        .map(ProducerMetadataKey::Stream),
                ),
                _ => {}
            }
        }
        required
    }

    fn key_ready(index: &ProducerStreamIndex, key: &ProducerMetadataKey) -> bool {
        index.loaded_metadata.contains(key)
            || match key {
                ProducerMetadataKey::Stream(stream) => index.streams.contains_key(stream),
                ProducerMetadataKey::Batch(stream, sequence) => {
                    index.streams.get(stream).is_some_and(|state| {
                        *sequence >= state.next_sequence || state.batches.contains_key(sequence)
                    })
                }
                ProducerMetadataKey::Position(stream, offset) => {
                    index.batch_positions.contains_key(&(*stream, *offset))
                }
                _ => false,
            }
    }

    #[tracing::instrument(name = "durable_stream.metadata.query", level = "debug", skip_all)]
    async fn index_for_query(
        &self,
        keys: Vec<ProducerMetadataKey>,
        terminal: Option<StreamId>,
        session: Option<StreamSessionKeyV1>,
    ) -> Result<MutexGuard<'_, ProducerStreamIndex>, DurableStreamProducerError> {
        loop {
            self.ensure_healthy()?;
            if let Ok(index) = self.index.try_lock() {
                self.ensure_healthy()?;
                let required = self.query_keys(&index, &keys, session.as_ref());
                // A query's complete dependency set must fit even when it exceeds the cache budget.
                let budget = 128.max(required.len());
                let bounded = index.loaded_metadata.len() <= budget
                    && index.batch_positions.len() <= budget
                    && index.streams.len() <= budget;
                let metadata_ready = self.control_metadata_provider.get().is_none()
                    || index.complete_for_deletion
                    || bounded && required.iter().all(|key| Self::key_ready(&index, key));
                let terminal_ready = terminal.is_none_or(|id| {
                    index
                        .streams
                        .get(&id)
                        .is_none_or(|state| !state.terminal || state.terminal_event.is_some())
                });
                if metadata_ready && terminal_ready {
                    return Ok(index);
                }
            } else {
                // A store-polled waiter must not reserve the mutex permit. Enqueuing another
                // hydration task here can also keep competing callers permanently queued.
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                continue;
            }
            let producer = self
                .self_weak
                .upgrade()
                .expect("live producer has an owning Arc");
            let keys = keys.clone();
            let session = session.clone();
            let activity = self
                .durable_activity
                .inherit_or_enter()
                .ok_or(DurableStreamProducerError::RecoveryRequired)?;
            // Never return a guard in the task result: a suspended store may stop polling
            // its JoinHandle indefinitely. The task owns and releases all hydration locks.
            tokio::spawn(activity.scope(async move {
                let mut index = producer.index.lock().await;
                producer.ensure_healthy()?;
                if !index.complete_for_deletion
                    && producer.control_metadata_provider.get().is_some()
                    && (index.loaded_metadata.len() > 128
                        || index.batch_positions.len() > 128
                        || index.streams.len() > 128)
                {
                    *index = ProducerStreamIndex::default();
                    producer
                        .buses
                        .write()
                        .expect("durable stream bus map lock poisoned")
                        .retain(|_, bus| {
                            Arc::strong_count(bus) > 1 || bus.has_pending_deferred_terminal()
                        });
                }
                producer.load_index_keys(&mut index, keys.clone()).await?;
                let required = producer.query_keys(&index, &keys, session.as_ref());
                producer.load_index_keys(&mut index, required).await?;
                if let Some(stream) = terminal {
                    producer.load_terminal(&mut index, stream).await?;
                }
                Ok::<(), DurableStreamProducerError>(())
            }))
            .await
            .map_err(|error| DurableStreamProducerError::Oplog(error.to_string()))??;
        }
    }

    pub(super) async fn index_for_cleanup(
        &self,
    ) -> Result<MutexGuard<'_, ProducerStreamIndex>, DurableStreamProducerError> {
        let mut index = self.index.lock().await;
        self.ensure_healthy()?;
        if self.control_metadata_provider.get().is_some() && !index.complete_for_deletion {
            let mut complete = Self::read_complete_index(
                self.oplog.as_ref(),
                self.environment_id,
                &self.producer,
                self.producer_fingerprint,
            )
            .await?;
            complete.complete_for_deletion = complete.deleting;
            for (stream, state) in &complete.streams {
                self.buses
                    .write()
                    .expect("durable stream bus map lock poisoned")
                    .entry(*stream)
                    .or_insert(Arc::new(DurableLiveStreamBus::from_committed_high_water(
                        self.live_join_capacity,
                        state.last_offset,
                    )?));
            }
            *index = complete;
        }
        Ok(index)
    }

    pub(super) async fn load_index_keys(
        &self,
        index: &mut ProducerStreamIndex,
        keys: impl IntoIterator<Item = ProducerMetadataKey> + Send,
    ) -> Result<(), DurableStreamProducerError> {
        if index.complete_for_deletion {
            return Ok(());
        }
        let result = self.try_load_index_keys(index, keys).await;
        if result.is_err() {
            *index = ProducerStreamIndex::default();
            self.buses
                .write()
                .expect("durable stream bus map lock poisoned")
                .retain(|_, bus| Arc::strong_count(bus) > 1 || bus.has_pending_deferred_terminal());
        }
        result
    }

    async fn try_load_index_keys(
        &self,
        index: &mut ProducerStreamIndex,
        keys: impl IntoIterator<Item = ProducerMetadataKey> + Send,
    ) -> Result<(), DurableStreamProducerError> {
        let Some((service, mode)) = self.control_metadata_provider.get() else {
            return Ok(());
        };
        let owner = OwnedAgentId::new(self.environment_id, &self.producer);
        let mut pending: Vec<_> = keys.into_iter().collect();
        pending.push(ProducerMetadataKey::Global);
        while !pending.is_empty() {
            let mut visiting = std::mem::take(&mut pending);
            let mut seen = HashSet::new();
            let mut batch = Vec::new();
            while let Some(key) = visiting.pop() {
                if !seen.insert(key.clone()) {
                    continue;
                }
                let dependency = match &key {
                    ProducerMetadataKey::Batch(stream, _)
                    | ProducerMetadataKey::Attachment(_, stream) => {
                        Some(ProducerMetadataKey::Stream(*stream))
                    }
                    ProducerMetadataKey::Stream(stream) => index
                        .stream_sessions
                        .get(stream)
                        .cloned()
                        .map(ProducerMetadataKey::Session),
                    ProducerMetadataKey::Coordinate(coordinate) => index
                        .coordinates
                        .get(coordinate)
                        .copied()
                        .map(ProducerMetadataKey::Stream),
                    _ => None,
                };
                if let Some(dependency) = dependency
                    && !Self::key_ready(index, &dependency)
                {
                    pending.push(key);
                    visiting.push(dependency);
                    continue;
                }
                if Self::key_ready(index, &key) {
                    continue;
                }
                if let ProducerMetadataKey::Position(stream, offset) = &key
                    && index.batch_positions.contains_key(&(*stream, *offset))
                {
                    index.loaded_metadata.insert(key);
                    continue;
                }
                batch.push(key);
            }
            if batch.is_empty() {
                continue;
            }
            let (_, rows) = service
                .lookup_durable_stream_producer_metadata(&owner, *mode, batch.clone())
                .await
                .map_err(DurableStreamProducerError::Oplog)?;
            if rows.len() != batch.len() {
                return Err(DurableStreamProducerError::CorruptHistory(
                    "producer metadata lookup returned an incorrect row count".into(),
                ));
            }
            for (key, row) in batch.into_iter().zip(rows) {
                if let Some(row) = row {
                    match &row {
                        ProducerMetadataRow::Coordinate(stream) => {
                            pending.push(ProducerMetadataKey::Stream(*stream));
                        }
                        ProducerMetadataRow::Stream(stream) => {
                            pending.push(ProducerMetadataKey::Session(stream.session_key.clone()));
                            self.buses
                                .write()
                                .expect("durable stream bus map lock poisoned")
                                .entry(stream.registration.handle.stream_id)
                                .or_insert(Arc::new(
                                    DurableLiveStreamBus::from_committed_high_water(
                                        self.live_join_capacity,
                                        stream.last_offset,
                                    )?,
                                ));
                        }
                        _ => {}
                    }
                    index
                        .hydrate_metadata_row(key.clone(), row)
                        .map_err(DurableStreamProducerError::CorruptHistory)?;
                }
                index.loaded_metadata.insert(key);
            }
        }
        Ok(())
    }

    pub(super) async fn indexed_attachment_candidates(
        &self,
        batch_size: usize,
    ) -> Result<
        Option<(bool, Vec<(IndexedStreamAttachment, ProducerJournalSummary)>)>,
        DurableStreamProducerError,
    > {
        let producer = self
            .self_weak
            .upgrade()
            .expect("live producer has an owning Arc");
        let activity = self
            .durable_activity
            .inherit_or_enter()
            .ok_or(DurableStreamProducerError::RecoveryRequired)?;
        tokio::spawn(activity.scope(async move {
            producer
                .indexed_attachment_candidates_inner(batch_size)
                .await
        }))
        .await
        .map_err(|error| DurableStreamProducerError::Oplog(error.to_string()))?
    }

    async fn indexed_attachment_candidates_inner(
        &self,
        batch_size: usize,
    ) -> Result<
        Option<(bool, Vec<(IndexedStreamAttachment, ProducerJournalSummary)>)>,
        DurableStreamProducerError,
    > {
        let Some((service, mode)) = self.control_metadata_provider.get() else {
            return Ok(None);
        };
        let _guard = self.index.lock().await;
        let owner = OwnedAgentId::new(self.environment_id, &self.producer);
        let (_, mut rows) = service
            .lookup_durable_stream_producer_metadata(
                &owner,
                *mode,
                vec![ProducerMetadataKey::Global],
            )
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        let Some(ProducerMetadataRow::Global {
            active_attachments: count,
            deleting,
            ..
        }) = rows.pop().flatten()
        else {
            return Ok(Some((false, Vec::new())));
        };
        if count == 0 || batch_size == 0 {
            return Ok(Some((deleting, Vec::new())));
        }
        let start = self
            .reconciliation_cursor
            .fetch_add(batch_size, Ordering::Relaxed) as u64
            % count;
        let positions = (0..(batch_size as u64).min(count))
            .map(|n| (start + n) % count)
            .collect::<Vec<_>>();
        let pages = positions
            .iter()
            .map(|n| n / ATTACHMENT_PAGE_SIZE)
            .collect::<std::collections::BTreeSet<_>>();
        let (_, rows) = service
            .lookup_durable_stream_producer_metadata(
                &owner,
                *mode,
                pages
                    .iter()
                    .map(|n| ProducerMetadataKey::AttachmentPage(*n))
                    .collect(),
            )
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        let mut catalogue = HashMap::new();
        for (page, row) in pages.into_iter().zip(rows) {
            let Some(ProducerMetadataRow::AttachmentPage(slots)) = row else {
                return Err(DurableStreamProducerError::CorruptHistory(
                    "active attachment catalogue page is missing".into(),
                ));
            };
            catalogue.insert(page, slots);
        }
        let mut keys = Vec::new();
        for position in positions {
            let (attachment, stream) = catalogue
                .get(&(position / ATTACHMENT_PAGE_SIZE))
                .and_then(|page| page.get((position % ATTACHMENT_PAGE_SIZE) as usize))
                .ok_or_else(|| {
                    DurableStreamProducerError::CorruptHistory(
                        "active attachment catalogue slot is missing".into(),
                    )
                })?;
            keys.push(ProducerMetadataKey::Attachment(*attachment, *stream));
            keys.push(ProducerMetadataKey::Stream(*stream));
        }
        let (_, rows) = service
            .lookup_durable_stream_producer_metadata(&owner, *mode, keys)
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        let mut candidates = Vec::new();
        for pair in rows.as_chunks::<2>().0 {
            let [
                Some(ProducerMetadataRow::Attachment(attachment)),
                Some(ProducerMetadataRow::Stream(stream)),
            ] = pair
            else {
                return Err(DurableStreamProducerError::CorruptHistory(
                    "active attachment catalogue refers to missing metadata".into(),
                ));
            };
            candidates.push((
                attachment.clone(),
                ProducerJournalSummary {
                    event_count: stream.next_sequence + u64::from(stream.terminal),
                    last_offset: stream.last_offset,
                    terminal: stream.terminal,
                },
            ));
        }
        Ok(Some((deleting, candidates)))
    }

    pub(crate) async fn journal_lag_events(
        &self,
        handle: &DurableStreamHandleV1,
        after: Option<StreamOffsetV1>,
    ) -> Result<usize, DurableStreamProducerError> {
        let mut keys = vec![ProducerMetadataKey::Stream(handle.stream_id)];
        if let Some(after) = after {
            keys.push(ProducerMetadataKey::Position(
                handle.stream_id,
                after.producer_oplog_index(),
            ));
        }
        let (stream, position) = if self.owns_handle_identity(handle) {
            let index = self.index_for(keys).await?;
            let Some(ProducerMetadataRow::Stream(stream)) =
                index.metadata_row(&ProducerMetadataKey::Stream(handle.stream_id))
            else {
                return Err(DurableStreamProducerError::InvalidHandle);
            };
            let position = after.and_then(|after| {
                index
                    .batch_positions
                    .get(&(handle.stream_id, after.producer_oplog_index()))
                    .copied()
            });
            (stream, position)
        } else {
            let (service, _) = self.control_metadata_provider.get().ok_or_else(|| {
                DurableStreamProducerError::Oplog("producer metadata service is unavailable".into())
            })?;
            let owner = OwnedAgentId::new(handle.producer_environment_id, &handle.producer);
            let mode = service
                .get_agent_mode(&owner)
                .await
                .ok_or(DurableStreamProducerError::InvalidHandle)?;
            let (_, rows) = service
                .lookup_durable_stream_producer_metadata(&owner, mode, keys)
                .await
                .map_err(DurableStreamProducerError::Oplog)?;
            let mut rows = rows.into_iter();
            let Some(Some(ProducerMetadataRow::Stream(stream))) = rows.next() else {
                return Err(DurableStreamProducerError::InvalidHandle);
            };
            let position = match rows.next().flatten() {
                Some(ProducerMetadataRow::Position(first, count)) => Some((first, count)),
                _ => None,
            };
            (stream, position)
        };
        if stream.registration.handle != *handle {
            return Err(DurableStreamProducerError::InvalidHandle);
        }
        let total = stream
            .next_sequence
            .checked_add(u64::from(stream.terminal))
            .ok_or(DurableStreamProducerError::CounterOverflow)?;
        let consumed = match after {
            None => 0,
            Some(after) if stream.terminal && stream.last_offset == Some(after) => total,
            Some(after) => {
                let (first, count) =
                    position.ok_or(DurableStreamProducerError::CursorUnavailable)?;
                if u64::from(after.sub_index()) >= count
                    || stream.last_offset.is_none_or(|last| after > last)
                {
                    return Err(DurableStreamProducerError::CursorUnavailable);
                }
                first
                    .checked_add(u64::from(after.sub_index()) + 1)
                    .ok_or(DurableStreamProducerError::CounterOverflow)?
            }
        };
        usize::try_from(
            total
                .checked_sub(consumed)
                .ok_or(DurableStreamProducerError::CursorUnavailable)?,
        )
        .map_err(|_| DurableStreamProducerError::CounterOverflow)
    }

    pub(super) async fn load_terminal(
        &self,
        index: &mut ProducerStreamIndex,
        stream_id: StreamId,
    ) -> Result<(), DurableStreamProducerError> {
        for (id, stream) in &mut index.streams {
            if *id != stream_id {
                stream.terminal_event = None;
            }
        }
        let stream = index
            .streams
            .get_mut(&stream_id)
            .ok_or(DurableStreamProducerError::UnknownStream(stream_id))?;
        if !stream.terminal || stream.terminal_event.is_some() {
            return Ok(());
        }
        let offset = stream.last_offset.ok_or_else(|| {
            DurableStreamProducerError::CorruptHistory(
                "terminal stream metadata has no durable offset".into(),
            )
        })?;
        let event = read_terminal_event(self.oplog.as_ref(), stream_id, offset).await?;
        if event.producer_sequence != stream.next_sequence {
            return Err(DurableStreamProducerError::CorruptHistory(
                "terminal metadata does not match its durable record".into(),
            ));
        }
        stream.terminal_event = Some(event);
        Ok(())
    }
}

pub(super) async fn read_terminal_event(
    oplog: &dyn Oplog,
    stream_id: StreamId,
    offset: StreamOffsetV1,
) -> Result<CommittedProducerStreamEventV1, DurableStreamProducerError> {
    let (id, sequence, recorded_offset, author, payload) =
        match oplog.read(offset.producer_oplog_index()).await {
            OplogEntry::StreamEnd { record, .. } => {
                let record = oplog
                    .download_payload(record)
                    .await
                    .map_err(DurableStreamProducerError::Oplog)?;
                (
                    record.stream_id,
                    record.sequence,
                    record.offset,
                    record.authored_by,
                    CommittedProducerStreamEventPayloadV1::End(record.result),
                )
            }
            OplogEntry::StreamCancel { record, .. } => {
                let record = oplog
                    .download_payload(record)
                    .await
                    .map_err(DurableStreamProducerError::Oplog)?;
                (
                    record.stream_id,
                    record.sequence,
                    record.offset,
                    record.authored_by,
                    CommittedProducerStreamEventPayloadV1::Cancel {
                        role: record.role,
                        reason: record.reason,
                        details: record.details,
                    },
                )
            }
            _ => {
                return Err(DurableStreamProducerError::CorruptHistory(
                    "terminal metadata points at a non-terminal record".into(),
                ));
            }
        };
    if id != stream_id || recorded_offset != offset {
        return Err(DurableStreamProducerError::CorruptHistory(
            "terminal metadata does not match its durable record".into(),
        ));
    }
    Ok(CommittedProducerStreamEventV1 {
        stream_id,
        producer_sequence: sequence,
        offset,
        packed_u8_batch_end: None,
        terminal_author: Some(author),
        nested_handles: Vec::new(),
        payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable_host::durable_stream::tests::{TestIdentity, identity, registration};
    use crate::model::ExecutionStatus;
    use crate::services::golem_config::GolemConfig;
    use crate::services::oplog::tests::{ReadCountingBlobStorage, ReadCountingIndexedStorage};
    use crate::services::oplog::{
        CompressedOplogArchiveService, MultiLayerOplog, MultiLayerOplogService, OplogService,
        PrimaryOplogService,
    };
    use crate::services::shard::ShardServiceDefault;
    use crate::services::worker::DefaultWorkerService;
    use crate::services::worker::session_index_tests::UnusedComponentService;
    use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::durable_stream::StreamRootKindV1;
    use golem_common::model::{AgentMetadata, AgentStatusRecord, RetryConfig, Timestamp};
    use golem_common::read_only_lock;
    use test_r::{test, timeout};

    struct Fixture {
        identity: TestIdentity,
        service: Arc<DefaultWorkerService>,
        oplog: Arc<dyn Oplog>,
        indexed: Arc<ReadCountingIndexedStorage>,
        blobs: Arc<ReadCountingBlobStorage>,
    }

    impl Fixture {
        async fn new() -> Self {
            Self::with_archive(false).await
        }

        async fn with_archive(archive: bool) -> Self {
            let identity = identity();
            let owner = OwnedAgentId::new(identity.environment_id, &identity.agent_id);
            let indexed = Arc::new(ReadCountingIndexedStorage::new());
            let blobs = Arc::new(ReadCountingBlobStorage::new());
            let primary = Arc::new(
                PrimaryOplogService::new(
                    indexed.clone(),
                    blobs.clone(),
                    1,
                    1,
                    100,
                    RetryConfig::default(),
                )
                .await,
            );
            let primary: Arc<dyn OplogService> = if archive {
                Arc::new(MultiLayerOplogService::new(
                    primary,
                    nonempty_collections::nev![Arc::new(CompressedOplogArchiveService::new(
                        indexed.clone(),
                        1,
                        RetryConfig::default()
                    ))
                        as Arc<dyn crate::services::oplog::OplogArchiveService>],
                    10_000,
                    10_000,
                ))
            } else {
                primary
            };
            let account = AccountId::new();
            let metadata = AgentMetadata {
                agent_id: identity.agent_id.clone(),
                env: vec![],
                environment_id: identity.environment_id,
                created_by: account,
                created_by_email: AccountEmail::new("producer-index@test"),
                config: vec![],
                created_at: Timestamp::now_utc(),
                parent: None,
                last_known_status: AgentStatusRecord::default(),
                original_phantom_id: None,
                fingerprint: identity.fingerprint,
                agent_mode: AgentMode::Durable,
            };
            let create = OplogEntry::create(
                identity.agent_id.clone(),
                AgentMode::Durable,
                golem_common::model::component::ComponentRevision::INITIAL,
                vec![],
                identity.environment_id,
                account,
                None,
                100,
                100,
                HashSet::new(),
                vec![],
                None,
                identity.fingerprint.0,
            );
            let oplog = primary
                .create_fresh(
                    &owner,
                    AgentMode::Durable,
                    create,
                    metadata,
                    read_only_lock::arc_swap::ReadOnlyView::new(Arc::new(
                        arc_swap::ArcSwap::from_pointee(AgentStatusRecord::default()),
                    )),
                    read_only_lock::std::ReadOnlyLock::new(Arc::new(RwLock::new(
                        ExecutionStatus::Suspended {
                            agent_mode: AgentMode::Durable,
                            timestamp: Timestamp::now_utc(),
                        },
                    ))),
                )
                .await;
            let service = Arc::new(DefaultWorkerService::new(
                Arc::new(InMemoryKeyValueStorage::new()),
                Arc::new(ShardServiceDefault::new()),
                primary,
                Arc::new(UnusedComponentService),
                Arc::new(GolemConfig::default()),
            ));
            Self {
                identity,
                service,
                oplog,
                indexed,
                blobs,
            }
        }

        async fn producer(&self) -> Arc<DurableStreamProducer> {
            let oplog = self.oplog.clone();
            let commit: DurableStreamCommit = Arc::new(move |published| {
                let oplog = oplog.clone();
                Box::pin(async move {
                    oplog.commit(CommitLevel::Always).await;
                    if let Some(published) = published {
                        let _ = published.send(());
                    }
                })
            });
            DurableStreamProducer::load_indexed_with_commit(
                self.oplog.clone(),
                OwnedAgentId::new(self.identity.environment_id, &self.identity.agent_id),
                self.identity.fingerprint,
                None,
                commit,
                self.service.clone(),
                AgentMode::Durable,
            )
            .await
            .unwrap()
        }

        fn registration(&self, ordinal: u32) -> ProducerRegistrationRequestV1 {
            registration(
                &self.identity,
                StreamRegistrationCoordinateV1::Root {
                    invocation_id: self.identity.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodResult,
                    recursive_value_path: vec![
                        golem_common::model::durable_stream::StreamValuePathStepV1::TupleElement(
                            ordinal,
                        ),
                    ],
                },
                StreamSourceKindV1::InvocationOutput,
            )
        }

        fn input_registration(&self, ordinal: u32) -> ProducerRegistrationRequestV1 {
            registration(
                &self.identity,
                StreamRegistrationCoordinateV1::Root {
                    invocation_id: self.identity.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodInput,
                    recursive_value_path: vec![
                        golem_common::model::durable_stream::StreamValuePathStepV1::TupleElement(
                            ordinal,
                        ),
                    ],
                },
                StreamSourceKindV1::ExternalInlineInput,
            )
        }

        async fn persist(&self) {
            let owner = OwnedAgentId::new(self.identity.environment_id, &self.identity.agent_id);
            self.oplog.commit(CommitLevel::Always).await;
            self.service
                .lookup_durable_stream_producer_metadata(
                    &owner,
                    AgentMode::Durable,
                    vec![ProducerMetadataKey::Global],
                )
                .await
                .unwrap();
            self.indexed.reset();
            self.blobs.reset();
        }
    }

    #[test]
    #[timeout("60s")]
    async fn external_producer_offsets_survive_indexed_cold_load() {
        let fixture = Fixture::new().await;
        let producer = fixture.producer().await;
        let handle = producer
            .register(fixture.input_registration(0))
            .await
            .unwrap()
            .value;
        let request = ExternalProducerV1 {
            id: "indexed".into(),
            epoch: 3,
            sequence: 0,
        };
        let accepted = producer
            .append_external_input(
                &fixture.identity.invocation,
                handle.stream_id,
                Some(StreamItemsPayloadV1::PackedU8(vec![7])),
                false,
                Some(request.clone()),
            )
            .await
            .unwrap();
        let ExternalAppendOutcomeV1::Accepted(offset) = accepted else {
            panic!()
        };
        fixture.persist().await;
        drop(producer);

        let cold = fixture.producer().await;
        assert_eq!(
            cold.append_external_input(
                &fixture.identity.invocation,
                handle.stream_id,
                Some(StreamItemsPayloadV1::PackedU8(vec![7])),
                false,
                Some(request),
            )
            .await
            .unwrap(),
            ExternalAppendOutcomeV1::Duplicate(offset)
        );
    }

    #[test]
    #[timeout("60s")]
    async fn persisted_finished_session_rejects_new_events_after_cold_load_and_eviction() {
        let fixture = Fixture::new().await;
        let producer = fixture.producer().await;
        let session = fixture.identity.invocation.clone();
        producer
            .ensure_session_accepts_new_events(&session)
            .await
            .unwrap();
        producer
            .finish_session(session.clone(), Ok(()), StreamCancelReasonV1::Protocol)
            .await
            .unwrap();
        fixture.persist().await;
        drop(producer);

        let cold = fixture.producer().await;
        assert!(cold.index.lock().await.finished_sessions.is_empty());
        let tip = fixture.oplog.current_oplog_index().await;
        assert_eq!(
            cold.ensure_session_accepts_new_events(&session).await,
            Err(DurableStreamProducerError::SessionFinished(session.clone()))
        );
        let reads = fixture.indexed.reads();
        for _ in 0..3 {
            assert_eq!(
                cold.ensure_session_accepts_new_events(&session).await,
                Err(DurableStreamProducerError::SessionFinished(session.clone()))
            );
        }
        assert_eq!(fixture.indexed.reads(), reads, "warm checks perform no IO");

        for ordinal in 0..130 {
            let mut other = session.clone();
            other.idempotency_key =
                golem_common::model::IdempotencyKey::new(format!("open-session-{ordinal}"));
            cold.ensure_session_accepts_new_events(&other)
                .await
                .unwrap();
        }
        assert!(!cold.index.lock().await.finished_sessions.contains(&session));
        assert_eq!(
            cold.ensure_session_accepts_new_events(&session).await,
            Err(DurableStreamProducerError::SessionFinished(session))
        );
        assert_eq!(fixture.oplog.current_oplog_index().await, tip);
        assert_eq!(
            fixture.blobs.reads(),
            0,
            "checks do not load payload history"
        );
    }

    #[test]
    #[timeout("60s")]
    async fn persisted_producer_cold_load_pages_only_requested_payloads() {
        let fixture = Fixture::new().await;
        let producer = fixture.producer().await;
        let handle = producer
            .register(fixture.registration(0))
            .await
            .unwrap()
            .value;
        let mut offsets = Vec::new();
        for sequence in 0..140 {
            let outcome = producer
                .write_items(
                    handle.stream_id,
                    sequence,
                    StreamItemsPayloadV1::Values(vec![vec![sequence as u8; 128]]),
                )
                .await
                .unwrap();
            offsets.push(outcome.value[0]);
        }
        for _ in 0..2100 {
            fixture.oplog.add(OplogEntry::interrupted()).await;
        }
        fixture.persist().await;
        drop(producer);
        let cold = fixture.producer().await;
        assert_eq!(
            fixture.indexed.reads(),
            1,
            "cold load only asks for the committed tip"
        );
        assert_eq!(
            fixture.blobs.reads(),
            0,
            "cold load must not download payload history"
        );
        assert!(cold.index.lock().await.streams.is_empty());
        let high_water = cold
            .input_high_water(handle.stream_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(high_water.highest_contiguous_sequence, 139);
        let metadata_reads = fixture.indexed.reads();
        assert_eq!(fixture.blobs.reads(), 0);
        for _ in 0..20 {
            cold.input_high_water(handle.stream_id).await.unwrap();
        }
        assert_eq!(
            fixture.indexed.reads(),
            metadata_reads,
            "warm metadata performs no storage IO"
        );
        let first = cold
            .read_segment(&handle, None, Some(offsets[0]))
            .await
            .unwrap();
        assert_eq!(first.len(), 1, "catch-up stops at the requested horizon");
        assert_eq!(fixture.blobs.reads(), 1);
        let second = cold
            .read_segment(&handle, Some(first[0].offset), Some(offsets[1]))
            .await
            .unwrap();
        assert_eq!(second[0].producer_sequence, 1);
        assert_eq!(fixture.blobs.reads(), 2);
        assert!(
            cold.index.lock().await.streams[&handle.stream_id]
                .batches
                .len()
                <= 128
        );
        let mut after = Some(second[0].offset);
        for sequence in 2..140 {
            let page = cold
                .read_segment(&handle, after, Some(offsets[sequence as usize]))
                .await
                .unwrap();
            assert_eq!(page[0].producer_sequence, sequence);
            after = Some(page[0].offset);
            assert!(cold.index.lock().await.batch_positions.len() <= 129);
        }
        assert_eq!(
            fixture.blobs.reads(),
            140,
            "each requested payload is read once"
        );
        let replay = cold
            .write_items(
                handle.stream_id,
                0,
                StreamItemsPayloadV1::Values(vec![vec![0; 128]]),
            )
            .await
            .unwrap();
        assert_eq!(replay.value, vec![first[0].offset]);
    }

    #[test]
    #[timeout("60s")]
    async fn cold_packed_cursor_reads_the_enclosing_batch_and_traverses_pages() {
        let fixture = Fixture::new().await;
        let producer = fixture.producer().await;
        let handle = producer
            .register(fixture.registration(0))
            .await
            .unwrap()
            .value;
        let offsets = producer
            .write_items(
                handle.stream_id,
                0,
                StreamItemsPayloadV1::PackedU8((0..5000).map(|index| index as u8).collect()),
            )
            .await
            .unwrap()
            .value;
        fixture.persist().await;
        drop(producer);
        let cold = fixture.producer().await;

        let mut after = Some(offsets[9]);
        let mut sequences = Vec::new();
        loop {
            let page = cold.read_segment(&handle, after, None).await.unwrap();
            if page.is_empty() {
                break;
            }
            assert!(page.len() <= 256);
            if sequences.is_empty() {
                assert_eq!(
                    page.iter()
                        .map(|event| event.producer_sequence)
                        .collect::<Vec<_>>(),
                    (10..266).collect::<Vec<_>>()
                );
            }
            sequences.extend(page.iter().map(|event| event.producer_sequence));
            after = page.last().map(|event| event.offset);
        }
        assert_eq!(sequences, (10..5000).collect::<Vec<_>>());
        assert_eq!(
            fixture.blobs.reads(),
            20,
            "one packed payload read per page"
        );
    }

    #[test]
    #[timeout("60s")]
    async fn cold_packed_batches_remain_contiguous_and_respect_exact_through() {
        let fixture = Fixture::new().await;
        let producer = fixture.producer().await;
        let handle = producer
            .register(fixture.registration(0))
            .await
            .unwrap()
            .value;
        let first = producer
            .write_items(
                handle.stream_id,
                0,
                StreamItemsPayloadV1::PackedU8(vec![1; 64]),
            )
            .await
            .unwrap()
            .value;
        let second = producer
            .write_items(
                handle.stream_id,
                64,
                StreamItemsPayloadV1::PackedU8(vec![2; 64]),
            )
            .await
            .unwrap()
            .value;
        fixture.persist().await;
        drop(producer);
        let cold = fixture.producer().await;

        let page = cold
            .read_segment(&handle, Some(first[9]), None)
            .await
            .unwrap();
        assert_eq!(
            page.iter()
                .map(|event| event.producer_sequence)
                .collect::<Vec<_>>(),
            (10..128).collect::<Vec<_>>()
        );
        assert_eq!(
            page[54].offset, second[0],
            "the next batch starts at sequence 64"
        );

        fixture.blobs.reset();
        let exact = cold
            .read_segment(&handle, Some(first[9]), Some(first[63]))
            .await
            .unwrap();
        assert_eq!(exact.len(), 54);
        assert_eq!(exact.last().unwrap().offset, first[63]);
        assert_eq!(
            fixture.blobs.reads(),
            1,
            "through does not read the next payload"
        );
    }

    #[test]
    #[timeout("60s")]
    async fn cold_ordinary_segment_loads_batch_locators_in_linear_windows() {
        let fixture = Fixture::new().await;
        let producer = fixture.producer().await;
        let handle = producer
            .register(fixture.registration(0))
            .await
            .unwrap()
            .value;
        for sequence in 0..256 {
            producer
                .write_items(
                    handle.stream_id,
                    sequence,
                    StreamItemsPayloadV1::Values(vec![vec![sequence as u8; 64]]),
                )
                .await
                .unwrap();
        }
        fixture.persist().await;
        drop(producer);
        let cold = fixture.producer().await;
        let indexed_reads = fixture.indexed.reads();
        let page = cold.read_segment(&handle, None, None).await.unwrap();
        assert_eq!(page.len(), 256);
        assert_eq!(
            page.iter()
                .map(|event| event.producer_sequence)
                .collect::<Vec<_>>(),
            (0..256).collect::<Vec<_>>()
        );
        assert_eq!(fixture.blobs.reads(), 256);
        // Each external payload requires one indexed oplog entry read, in addition
        // to the metadata reads that resolve the three locator windows.
        let locator_reads = fixture.indexed.reads() - indexed_reads - 256;
        assert!(
            locator_reads <= 6,
            "batch locator fields are loaded in fixed windows, got {locator_reads} reads"
        );
    }

    #[test]
    #[timeout("60s")]
    async fn deletion_complete_cold_fallback_preserves_full_locator_prefix() {
        let fixture = Fixture::new().await;
        let producer = fixture.producer().await;
        let handle = producer
            .register(fixture.registration(0))
            .await
            .unwrap()
            .value;
        let mut offsets = Vec::new();
        for sequence in 0..140 {
            offsets.push(
                producer
                    .write_items(
                        handle.stream_id,
                        sequence,
                        StreamItemsPayloadV1::Values(vec![vec![sequence as u8]]),
                    )
                    .await
                    .unwrap()
                    .value[0],
            );
        }
        let terminal_offset = producer
            .end(handle.stream_id, 140, StreamEndResultV1::Ok)
            .await
            .unwrap()
            .value;
        fixture.persist().await;
        drop(producer);

        let cold = fixture.producer().await;
        cold.commit_deletion_barrier(1_000, true).await.unwrap();
        assert!(cold.index.lock().await.complete_for_deletion);

        let page = cold.read_segment(&handle, None, None).await.unwrap();
        assert_eq!(page.len(), 141);
        assert_eq!(
            page.iter().map(|event| event.offset).collect::<Vec<_>>(),
            offsets
                .iter()
                .copied()
                .chain(std::iter::once(terminal_offset))
                .collect::<Vec<_>>()
        );
        assert!(page[..140].iter().enumerate().all(|(sequence, event)| {
            event.producer_sequence == sequence as u64
                && event.payload
                    == CommittedProducerStreamEventPayloadV1::Value(vec![sequence as u8])
        }));
        assert!(matches!(
            &page.last().unwrap().payload,
            CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::Ok)
        ));

        let index = cold.index.lock().await;
        assert!(index.complete_for_deletion);
        let stream = &index.streams[&handle.stream_id];
        assert_eq!(
            stream.batches,
            offsets
                .iter()
                .enumerate()
                .map(|(sequence, offset)| { (sequence as u64, offset.producer_oplog_index()) })
                .collect()
        );
        assert_eq!(
            stream.offsets(),
            offsets
                .iter()
                .copied()
                .chain(std::iter::once(terminal_offset))
                .collect::<Vec<_>>(),
            "deletion cascade must retain the exact producer offset prefix"
        );
        drop(index);
        cold.validate_cursor(handle.stream_id, Some(offsets[139]))
            .await
            .unwrap();
    }

    #[test]
    #[timeout("60s")]
    async fn archived_projection_extends_nested_chunk_and_replays_lazily() {
        let fixture = Fixture::with_archive(true).await;
        let producer = DurableStreamProducer::load(
            fixture.oplog.clone(),
            fixture.identity.environment_id,
            fixture.identity.agent_id.clone(),
            fixture.identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let handle = producer
            .register(fixture.registration(0))
            .await
            .unwrap()
            .value;
        for _ in 0..1021 {
            fixture.oplog.add(OplogEntry::interrupted()).await;
        }
        assert_eq!(fixture.oplog.current_oplog_index().await.as_u64(), 1023);
        let nested = registration(
            &fixture.identity,
            StreamRegistrationCoordinateV1::Nested {
                parent_stream_id: handle.stream_id,
                parent_producer_sequence: 0,
                recursive_value_path: vec![
                    golem_common::model::durable_stream::StreamValuePathStepV1::OptionSome,
                ],
            },
            StreamSourceKindV1::Nested,
        );
        producer
            .write_items_with_nested(
                handle.stream_id,
                0,
                StreamItemsPayloadV1::Values(vec![vec![1; 128]]),
                vec![nested],
            )
            .await
            .unwrap();
        let nested_handles = producer.nested_handles(handle.stream_id, 0).await.unwrap();
        fixture.oplog.commit(CommitLevel::Always).await;
        MultiLayerOplog::try_archive_blocking(&fixture.oplog)
            .await
            .expect("archive layer");
        fixture.persist().await;
        drop(producer);
        let cold = fixture.producer().await;
        assert_eq!(fixture.blobs.reads(), 0);
        let page = cold.read_segment(&handle, None, None).await.unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].nested_handles, nested_handles);
        assert_eq!(fixture.blobs.reads(), 1);
        assert_eq!(cold.index.lock().await.streams.len(), 2);
    }

    #[test]
    #[timeout("30s")]
    async fn cold_replay_allows_nested_dependencies_larger_than_cache_budget() {
        let fixture = Fixture::new().await;
        let producer = fixture.producer().await;
        let handle = producer
            .register(fixture.registration(0))
            .await
            .unwrap()
            .value;
        let nested = (0..64)
            .map(|ordinal| {
                registration(
                    &fixture.identity,
                    StreamRegistrationCoordinateV1::Nested {
                        parent_stream_id: handle.stream_id,
                        parent_producer_sequence: 0,
                        recursive_value_path: vec![
                            golem_common::model::durable_stream::StreamValuePathStepV1::TupleElement(ordinal),
                        ],
                    },
                    StreamSourceKindV1::Nested,
                )
            })
            .collect();
        producer
            .write_items_with_nested(
                handle.stream_id,
                0,
                StreamItemsPayloadV1::Values(vec![vec![1; 128]]),
                nested,
            )
            .await
            .unwrap();
        fixture.persist().await;
        drop(producer);
        let cold = fixture.producer().await;
        let page = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            cold.read_segment(&handle, None, None),
        )
        .await
        .expect("one operation may exceed the resident cache budget")
        .unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].nested_handles.len(), 64);
        let ids = page[0]
            .nested_handles
            .iter()
            .map(|handle| handle.stream_id)
            .collect::<Vec<_>>();
        let reads = fixture.indexed.reads();
        assert_eq!(
            cold.resolve_nested_handles(&ids).await.unwrap(),
            page[0].nested_handles
        );
        assert_eq!(
            fixture.indexed.reads(),
            reads,
            "the complete working set stays warm"
        );
        cold.input_high_water(handle.stream_id).await.unwrap();
        assert!(cold.index.lock().await.loaded_metadata.len() <= 128);
    }

    #[test]
    #[timeout("60s")]
    async fn persisted_reconciliation_pages_active_catalogue_without_payloads() {
        let fixture = Fixture::new().await;
        let producer = fixture.producer().await;
        let handle = producer
            .register(fixture.registration(0))
            .await
            .unwrap()
            .value;
        producer
            .write_items(handle.stream_id, 0, StreamItemsPayloadV1::PackedU8(vec![7]))
            .await
            .unwrap();
        let terminal_offset = producer
            .end(handle.stream_id, 1, StreamEndResultV1::Ok)
            .await
            .unwrap()
            .value;
        let mut attachments = Vec::new();
        for ordinal in 0..140 {
            let mut key = crate::durable_host::durable_stream::tests::attachment_key(
                &fixture.identity,
                handle.stream_id,
            );
            key.session_key.idempotency_key =
                golem_common::model::IdempotencyKey::new(format!("consumer-{ordinal}"));
            key.consumer_invocation = key.session_key.clone();
            key.attachment_id = AttachmentId::primary(
                key.consumer_environment_id,
                &key.consumer,
                &key.session_key.idempotency_key,
            )
            .unwrap();
            producer.prepare_attachment(key.clone(), 100).await.unwrap();
            attachments.push(key);
        }
        producer
            .finalize_attachment(
                attachments[0].clone(),
                StreamAttachmentFinalizationReasonV1::ConsumerFinalized,
                200,
            )
            .await
            .unwrap();
        fixture.persist().await;
        drop(producer);
        let cold = fixture.producer().await;
        let mut found = HashSet::new();
        for _ in 0..5 {
            let (_, page) = cold
                .indexed_attachment_candidates(32)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(page.len(), 32);
            for (attachment, summary) in page {
                assert_eq!(
                    summary,
                    ProducerJournalSummary {
                        event_count: 2,
                        last_offset: Some(terminal_offset),
                        terminal: true,
                    }
                );
                assert_ne!(attachment.key, attachments[0]);
                found.insert(attachment.key);
            }
        }
        assert_eq!(found, attachments.into_iter().skip(1).collect());
        assert_eq!(fixture.blobs.reads(), 0);
        assert!(
            cold.index.lock().await.streams.is_empty(),
            "reconciler must not hydrate runtime streams"
        );
    }

    #[test]
    #[timeout("60s")]
    async fn persisted_consumer_head_summarizes_long_journal_without_history_reads() {
        let fixture = Fixture::new().await;
        let consumer = fixture.producer().await;
        let stream_id = StreamId(uuid::Uuid::from_u128(91));
        let session_key = fixture.identity.invocation.clone();
        for ordinal in 0..512 {
            consumer
                .append_session_record(StreamSessionRecordV1::ConsumerItemValue(
                    golem_common::base_model::durable_stream::StreamConsumerItemValueRecordV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: session_key.clone(),
                        stream_id,
                        source_offset: StreamOffsetV1::new(OplogIndex::from_u64(ordinal + 1), 0),
                        consumer_read_ordinal: ordinal,
                        value: vec![ordinal as u8],
                        packed_u8: false,
                        recursive_handles: Vec::new(),
                        recursive_mappings: Vec::new(),
                    },
                ))
                .await
                .unwrap();
        }
        let terminal_offset = StreamOffsetV1::new(OplogIndex::from_u64(513), 0);
        consumer
            .append_session_record(StreamSessionRecordV1::ConsumerTerminal(
                golem_common::model::durable_stream::StreamConsumerTerminalRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: session_key.clone(),
                    stream_id,
                    source_offset: terminal_offset,
                    consumer_read_ordinal: 512,
                    terminal: golem_common::model::durable_stream::StreamConsumerTerminalV1::End(
                        StreamEndResultV1::Ok,
                    ),
                },
            ))
            .await
            .unwrap();
        fixture.persist().await;
        drop(consumer);

        let owner = OwnedAgentId::new(fixture.identity.environment_id, &fixture.identity.agent_id);
        let reads_before = fixture.indexed.reads();
        let (_, rows) = fixture
            .service
            .lookup_durable_stream_producer_metadata(
                &owner,
                AgentMode::Durable,
                vec![ProducerMetadataKey::ConsumerHead(session_key, stream_id)],
            )
            .await
            .unwrap();
        let Some(Some(ProducerMetadataRow::ConsumerHead(head))) = rows.into_iter().next() else {
            panic!("persisted consumer head is missing");
        };
        assert_eq!(head.next_read_ordinal, 513);
        assert_eq!(head.last_source_offset, Some(terminal_offset));
        assert!(head.terminal);
        assert!(head.source_unavailable.is_none());
        assert_eq!(fixture.indexed.reads(), reads_before + 1);
        assert_eq!(fixture.blobs.reads(), 0);
    }

    #[test]
    #[timeout("30s")]
    async fn cancelled_control_query_drains_detached_index_catch_up() {
        let fixture = Fixture::new().await;
        let producer = fixture.producer().await;
        let session = fixture.identity.invocation.clone();
        let stream = StreamId(uuid::Uuid::from_u128(93));
        let record = fixture
            .oplog
            .upload_payload(&StreamSessionRecordV1::ConsumerItemValue(
                golem_common::base_model::durable_stream::StreamConsumerItemValueRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: session.clone(),
                    stream_id: stream,
                    source_offset: StreamOffsetV1::new(OplogIndex::from_u64(17), 0),
                    consumer_read_ordinal: 0,
                    value: vec![9; 256],
                    packed_u8: false,
                    recursive_handles: Vec::new(),
                    recursive_mappings: Vec::new(),
                },
            ))
            .await
            .unwrap();
        assert!(matches!(record, OplogPayload::External { .. }));
        fixture.oplog.add(OplogEntry::stream_session(record)).await;
        fixture.oplog.commit(CommitLevel::Always).await;
        let horizon = fixture.oplog.current_oplog_index().await;
        let (started, release) = fixture.blobs.pause_next_read();
        let mut query = Box::pin(producer.persisted_control_metadata(&session));
        assert!(futures::poll!(query.as_mut()).is_pending());
        started.await.unwrap();
        drop(query);
        producer.durable_activity.close();
        let drained = producer.durable_activity.wait_drained();
        tokio::pin!(drained);
        assert!(futures::poll!(&mut drained).is_pending());
        release.send(()).unwrap();
        drained.await;

        let reads = fixture.indexed.reads();
        let owner = OwnedAgentId::new(fixture.identity.environment_id, &fixture.identity.agent_id);
        let metadata = fixture
            .service
            .lookup_durable_stream_control_metadata(&owner, AgentMode::Durable, &session)
            .await
            .unwrap();
        assert_eq!(metadata.covered_through, horizon);
        assert_eq!(metadata.consumer_record_counts.get(&stream), Some(&1));
        assert_eq!(
            fixture.indexed.reads(),
            reads + 1,
            "only the horizon lookup may read the oplog after drain, not another catch-up"
        );
        assert!(producer.persisted_control_metadata(&session).await.is_err());
    }

    #[test]
    #[timeout("30s")]
    async fn suspended_host_does_not_retain_producer_hydration_lock() {
        let fixture = Fixture::new().await;
        let producer = fixture.producer().await;
        let handle = producer
            .register(fixture.registration(0))
            .await
            .unwrap()
            .value;
        producer
            .end(
                handle.stream_id,
                0,
                StreamEndResultV1::ErrorContext(vec![7; 256]),
            )
            .await
            .unwrap();
        fixture.persist().await;
        drop(producer);
        let cold = fixture.producer().await;
        let (started, release) = fixture.blobs.pause_next_read();
        let mut suspended = Box::pin(cold.end(
            handle.stream_id,
            0,
            StreamEndResultV1::ErrorContext(vec![7; 256]),
        ));
        assert!(futures::poll!(suspended.as_mut()).is_pending());
        tokio::time::timeout(std::time::Duration::from_secs(5), started)
            .await
            .unwrap()
            .unwrap();
        release.send(()).unwrap();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            cold.input_high_water(handle.stream_id),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(fixture.blobs.reads(), 1);
        suspended.await.unwrap();
    }

    #[test]
    async fn suspended_contended_query_does_not_reserve_producer_index() {
        let fixture = Fixture::new().await;
        let producer = fixture.producer().await;
        let handle = producer
            .register(fixture.registration(0))
            .await
            .unwrap()
            .value;
        let held = producer.index.lock().await;
        let mut suspended = Box::pin(producer.input_high_water(handle.stream_id));
        assert!(futures::poll!(suspended.as_mut()).is_pending());
        drop(held);
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            producer.input_high_water(handle.stream_id),
        )
        .await
        .unwrap()
        .unwrap();
        suspended.await.unwrap();
    }

    #[test]
    async fn contended_warm_producer_queries_make_progress() {
        let fixture = Fixture::new().await;
        let producer = fixture.producer().await;
        let handle = producer
            .register(fixture.registration(0))
            .await
            .unwrap()
            .value;
        let held = producer.index.lock().await;
        let mut queries = (0..16)
            .map(|_| Box::pin(producer.input_high_water(handle.stream_id)))
            .collect::<Vec<_>>();
        for query in &mut queries {
            assert!(futures::poll!(query.as_mut()).is_pending());
        }
        drop(held);
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            futures::future::try_join_all(queries),
        )
        .await
        .unwrap()
        .unwrap();
    }
}
