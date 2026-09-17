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

#[cfg(test)]
use super::registration::registration_record;
use super::*;
use crate::storage::keyvalue::{
    KeyValueStorage, KeyValueStorageLabelledApi, KeyValueStorageNamespace,
};
use golem_common::model::durable_stream::StreamForkCutRecord;
use golem_common::serialization::serialize;

const ATTACHMENT_PAGE_SIZE: u64 = 128;
const SESSION_PAGE_SIZE: u64 = 128;

pub(super) struct IndexedAttachmentCandidate {
    pub(super) attachment: IndexedStreamAttachment,
    pub(super) journal_summary: ProducerJournalSummary,
}

pub(super) struct IndexedAttachmentCandidateBatch {
    pub(super) deleting: bool,
    pub(super) candidates: Vec<IndexedAttachmentCandidate>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, desert_rust::BinaryCodec)]
/// Stable metadata projection key derived from producer oplog records.
pub enum ProducerMetadataKey {
    Global,
    Lineage,
    Stream(StreamId),
    Coordinate(StreamRegistrationCoordinate),
    Reference(StreamId),
    Session(StreamSessionKey),
    SessionPage(u64),
    Batch(StreamId, u64),
    Attachment(AttachmentId, StreamId, EnvironmentId, AgentId),
    ActiveAttachmentCount(StreamSessionKey, StreamId),
    ConsumerHead(StreamSessionKey, StreamId),
    Cascade(Box<StreamAttachmentKey>),
    AttachmentPage(u64),
    AttachmentPosition(AttachmentId, StreamId, EnvironmentId, AgentId),
    Position(StreamId, OplogIndex),
    PrefixAlias(StreamId),
    ExternalProducerHead(StreamSessionKey, StreamId, ExternalProducerId),
    ExternalProducerSequence(StreamSessionKey, StreamId, ExternalProducerId, u64, u64),
}

impl ProducerMetadataKey {
    /// Encodes this key for the producer metadata store.
    pub fn field(&self) -> Result<String, String> {
        Ok(format!("producer:{}", hex::encode(serialize(self)?)))
    }

    pub(super) fn registration(request: &ProducerRegistrationRequest) -> Vec<Self> {
        let mut keys = vec![Self::Coordinate(request.coordinate.clone())];
        if let Some(mapping) = &request.session_mapping {
            keys.push(Self::Session(mapping.session_key.clone()));
        }
        match &request.coordinate {
            StreamRegistrationCoordinate::Root { invocation_id, .. } => {
                keys.push(Self::Session(invocation_id.clone()));
            }
            StreamRegistrationCoordinate::Nested {
                parent_stream_id, ..
            } => {
                keys.push(Self::Stream(*parent_stream_id));
            }
        }
        keys
    }

    pub(super) fn session_record(record: &StreamSessionRecord) -> Vec<Self> {
        let mut keys = Vec::new();
        if let Some(key) = crate::worker::stream_session_record_key(record) {
            keys.push(Self::Session(key.clone()));
        }
        let mappings = match record {
            StreamSessionRecord::Prepared(record) => record.stream_mappings.as_slice(),
            StreamSessionRecord::InvocationResult(record) => record.stream_mappings.as_slice(),
            StreamSessionRecord::Mapping(record) => std::slice::from_ref(&record.mapping),
            _ => &[],
        };
        keys.extend(
            mappings
                .iter()
                .map(|mapping| Self::Reference(mapping.handle.stream_id)),
        );
        let attachment = match record {
            StreamSessionRecord::AttachmentPrepared(record) => Some(&record.key),
            StreamSessionRecord::AttachmentActivated(record) => Some(&record.key),
            StreamSessionRecord::AttachmentRenewed(record) => Some(&record.key),
            StreamSessionRecord::AttachmentFinalized(record) => Some(&record.key),
            StreamSessionRecord::CascadeOutbox(record) => {
                keys.push(Self::Cascade(Box::new(record.key.clone())));
                Some(&record.key)
            }
            _ => None,
        };
        if let Some(key) = attachment {
            keys.push(Self::Attachment(
                key.attachment_id,
                key.stream_id,
                key.consumer_environment_id,
                key.consumer.clone(),
            ));
            keys.push(Self::ActiveAttachmentCount(
                key.session_key.clone(),
                key.stream_id,
            ));
            keys.push(Self::Stream(key.stream_id));
        }
        let head = match record {
            StreamSessionRecord::ConsumerItemValue(record) => {
                Some((&record.session_key, record.stream_id))
            }
            StreamSessionRecord::ConsumerTerminal(record) => {
                Some((&record.session_key, record.stream_id))
            }
            StreamSessionRecord::SourceUnavailable(record) => {
                Some((&record.key.session_key, record.key.stream_id))
            }
            _ => None,
        };
        if let Some((session, stream)) = head {
            keys.push(Self::ConsumerHead(session.clone(), stream));
        }
        if let StreamSessionRecord::ExternalProducerState(record) = record {
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
/// Projected registration and terminal metadata for one producer stream.
pub struct ProducerStreamMetadata {
    registration: StreamRegisteredRecord,
    session_key: StreamSessionKey,
    role: SessionStreamRole,
    entity_parent_start_index: Option<OplogIndex>,
    first_sequence: Option<u64>,
    next_sequence: u64,
    last_offset: Option<StreamOffset>,
    last_item_offset: Option<StreamOffset>,
    terminal: bool,
}

#[derive(Clone, desert_rust::BinaryCodec)]
/// Projected durable state for one stream session.
pub struct ProducerSessionMetadata {
    mappings: HashSet<(DurableStreamHandle, SessionStreamRole)>,
    references: HashSet<StreamId>,
    open_streams: HashSet<StreamId>,
    consumer_streams: HashSet<StreamId>,
    entity_parent_start_index: Option<OplogIndex>,
    invocation_result: Option<OplogIndex>,
    finished: bool,
}

#[derive(Clone, desert_rust::BinaryCodec)]
/// Typed row persisted in the producer metadata projection.
pub enum ProducerMetadataRow {
    Retired,
    Lineage(StreamForkLineage),
    Global {
        open_streams: usize,
        active_attachments: u64,
        session_count: u64,
        deleting: bool,
        consumer_deleting: bool,
    },
    Stream(Box<ProducerStreamMetadata>),
    Coordinate(StreamId),
    Reference(DurableStreamHandle),
    Session(ProducerSessionMetadata),
    SessionPage(Vec<StreamSessionKey>),
    Batch(OplogIndex),
    Attachment(IndexedStreamAttachment),
    ActiveAttachmentCount(u64),
    ConsumerHead(IndexedConsumerJournal),
    Cascade(StreamCascadeDependentResult),
    AttachmentPage(Vec<(AttachmentId, StreamId, EnvironmentId, AgentId)>),
    AttachmentPosition(Option<u64>),
    Position(u64, u64),
    PrefixAlias {
        previous_stream_id: StreamId,
        retained_next_sequence: u64,
    },
    ExternalProducer(IndexedExternalProducer),
    ExternalProducerOffset(StreamOffset),
}

/// Resolve historical data locations without loading a historical stream's authority.
/// Alias rows and locator rows remain in the current producer's metadata namespace.
pub(crate) async fn resolve_producer_locator(
    storage: &(dyn KeyValueStorage + Send + Sync),
    namespace: KeyValueStorageNamespace,
    key: &ProducerMetadataKey,
    mut row: Option<ProducerMetadataRow>,
    aliases: &mut HashMap<StreamId, Option<ProducerMetadataRow>>,
) -> Result<Option<ProducerMetadataRow>, String> {
    if matches!(row, Some(ProducerMetadataRow::Retired)) {
        return Ok(None);
    }
    let (mut stream, batch_sequence, position) = match key {
        ProducerMetadataKey::Batch(stream, sequence) => (*stream, Some(*sequence), None),
        ProducerMetadataKey::Position(stream, position) => (*stream, None, Some(*position)),
        _ => return Ok(row),
    };
    let mut visited = HashSet::new();
    let mut retained = u64::MAX;
    loop {
        if !visited.insert(stream) {
            return Err("producer metadata prefix alias cycle".into());
        }
        if let Some(row) = row {
            return match row {
                ProducerMetadataRow::Batch(_) if batch_sequence.is_some() => Ok(Some(row)),
                ProducerMetadataRow::Position(first, count) if position.is_some() => {
                    let end = first
                        .checked_add(count)
                        .ok_or("producer metadata position range overflow")?
                        .min(retained);
                    Ok((end > first).then_some(ProducerMetadataRow::Position(
                        first,
                        end.saturating_sub(first),
                    )))
                }
                _ => Err("producer metadata locator row type does not match its key".into()),
            };
        }
        let alias = match aliases.entry(stream) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.get().clone(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let alias = storage
                    .with_entity("worker", "producer_prefix_alias", "metadata")
                    .get::<ProducerMetadataRow>(
                        namespace.clone(),
                        &ProducerMetadataKey::PrefixAlias(stream).field()?,
                    )
                    .await?;
                entry.insert(alias).clone()
            }
        };
        let Some(alias) = alias else {
            return Ok(None);
        };
        let ProducerMetadataRow::PrefixAlias {
            previous_stream_id,
            retained_next_sequence,
        } = alias
        else {
            return Err("producer metadata prefix alias row type mismatch".into());
        };
        retained = retained.min(retained_next_sequence);
        if retained == 0 || batch_sequence.is_some_and(|sequence| sequence >= retained) {
            return Ok(None);
        }
        stream = previous_stream_id;
        let key = match (batch_sequence, position) {
            (Some(sequence), _) => ProducerMetadataKey::Batch(stream, sequence),
            (_, Some(position)) => ProducerMetadataKey::Position(stream, position),
            _ => unreachable!(),
        };
        row = storage
            .with_entity("worker", "producer_prefix_locator", "metadata")
            .get(namespace.clone(), &key.field()?)
            .await?;
    }
}

impl ProducerStreamIndex {
    pub(super) fn metadata_row(&self, key: &ProducerMetadataKey) -> Option<ProducerMetadataRow> {
        Some(match key {
            ProducerMetadataKey::PrefixAlias(_) => return None,
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
            ProducerMetadataKey::Reference(id) => {
                ProducerMetadataRow::Reference(self.referenced_handles.get(id)?.0.clone())
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
                ProducerMetadataRow::Attachment(
                    self.attachments
                        .get(&(*attachment, *stream, *environment, consumer.clone()))?
                        .clone(),
                )
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

    pub(super) fn hydrate_metadata_row(
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
            (ProducerMetadataKey::Reference(stream), ProducerMetadataRow::Reference(handle)) => {
                self.referenced_handles
                    .entry(stream)
                    .or_insert_with(|| (handle, HashSet::new()));
            }
            (
                ProducerMetadataKey::SessionPage(page),
                ProducerMetadataRow::SessionPage(sessions),
            ) => {
                self.session_pages.insert(page, sessions);
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
                    .insert((attachment, stream, environment, consumer), value);
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

struct Projection<'a> {
    index: ProducerStreamIndex,
    loaded: HashSet<ProducerMetadataKey>,
    new_sessions: HashSet<StreamSessionKey>,
    replacements: HashMap<ProducerMetadataKey, ProducerMetadataRow>,
    storage: &'a (dyn KeyValueStorage + Send + Sync),
    namespace: KeyValueStorageNamespace,
}

pub(crate) struct ForkControlProjection {
    pub(crate) source_sessions: Vec<StreamSessionKey>,
    pub(crate) retained_streams: HashSet<StreamId>,
}

#[derive(Default)]
pub(crate) struct ProducerMetadataProjection {
    pub(crate) rows: Vec<(String, Vec<u8>)>,
    pub(crate) fork: Option<ForkControlProjection>,
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
            } else if let ProducerMetadataKey::Session(session) = key {
                self.new_sessions.insert(session);
            }
        }
        Ok(())
    }

    async fn fork_cut(
        &mut self,
        cut: &StreamForkCutRecord,
    ) -> Result<ForkControlProjection, String> {
        for page in 0..self.index.session_count.div_ceil(SESSION_PAGE_SIZE) {
            self.load(ProducerMetadataKey::SessionPage(page)).await?;
            let sessions = self
                .index
                .session_pages
                .get(&page)
                .ok_or("producer session catalogue page is missing")?
                .clone();
            for session in sessions {
                self.load(ProducerMetadataKey::Session(session.clone()))
                    .await?;
                let streams = self
                    .index
                    .session_consumer_streams
                    .get(&session)
                    .cloned()
                    .unwrap_or_default();
                for stream in streams {
                    self.load(ProducerMetadataKey::ConsumerHead(session.clone(), stream))
                        .await?;
                }
            }
        }
        for mapping in &cut.streams {
            self.stream(mapping.source.stream_id).await?;
        }
        for page in 0..self
            .index
            .active_attachment_count
            .div_ceil(ATTACHMENT_PAGE_SIZE)
        {
            let key = ProducerMetadataKey::AttachmentPage(page);
            self.loaded.insert(key.clone());
            self.replacements
                .insert(key, ProducerMetadataRow::AttachmentPage(Vec::new()));
        }
        for coordinate in self.index.coordinates.keys() {
            self.loaded
                .insert(ProducerMetadataKey::Coordinate(coordinate.clone()));
        }
        for stream in self.index.referenced_handles.keys() {
            self.loaded.insert(ProducerMetadataKey::Reference(*stream));
        }
        let retained = match (cut.selected_stream_id, cut.retained_through) {
            (None, None) => None,
            (None, Some(_)) => return Err("fork retained offset has no selected stream".into()),
            (Some(_), None) => Some(0),
            (Some(stream), Some(offset)) => {
                let key = ProducerMetadataKey::Position(stream, offset.producer_oplog_index());
                let row = self
                    .storage
                    .with_entity("worker", "producer_projection_read", "position")
                    .get(self.namespace.clone(), &key.field()?)
                    .await?;
                let row = resolve_producer_locator(
                    self.storage,
                    self.namespace.clone(),
                    &key,
                    row,
                    &mut HashMap::new(),
                )
                .await?;
                let Some(ProducerMetadataRow::Position(first, count)) = row else {
                    return Err("fork retained offset is not an item of the selected stream".into());
                };
                let sub = u64::from(offset.sub_index());
                if sub >= count {
                    return Err("fork retained sub-offset exceeds its item batch".into());
                }
                Some(
                    first
                        .checked_add(sub)
                        .and_then(|n| n.checked_add(1))
                        .ok_or("fork retained item count overflow")?,
                )
            }
        };
        let source_sessions = self
            .index
            .session_entity_parent_start_indices
            .keys()
            .cloned()
            .collect();
        self.index
            .apply_fork_cut(cut, retained)
            .map_err(|error| error.to_string())?;
        // Locator rows are immutable history. Missing authority rows, unlike locators, must be
        // explicitly retired because the surrounding CAS only overwrites emitted fields.
        for key in &self.loaded {
            if matches!(
                key,
                ProducerMetadataKey::Stream(_)
                    | ProducerMetadataKey::Coordinate(_)
                    | ProducerMetadataKey::Reference(_)
                    | ProducerMetadataKey::Session(_)
                    | ProducerMetadataKey::ConsumerHead(_, _)
            ) && self.index.metadata_row(key).is_none()
            {
                self.replacements
                    .insert(key.clone(), ProducerMetadataRow::Retired);
            }
        }
        for mapping in &cut.streams {
            if let Some(state) = self.index.streams.get(&mapping.continuation.stream_id) {
                let key = ProducerMetadataKey::PrefixAlias(mapping.continuation.stream_id);
                self.loaded.insert(key.clone());
                self.replacements.insert(
                    key,
                    ProducerMetadataRow::PrefixAlias {
                        previous_stream_id: mapping.source.stream_id,
                        retained_next_sequence: state.next_sequence,
                    },
                );
            }
        }
        for stream in self.index.streams.keys() {
            self.loaded.insert(ProducerMetadataKey::Stream(*stream));
        }
        for coordinate in self.index.coordinates.keys() {
            self.loaded
                .insert(ProducerMetadataKey::Coordinate(coordinate.clone()));
        }
        for stream in self.index.referenced_handles.keys() {
            self.loaded.insert(ProducerMetadataKey::Reference(*stream));
        }
        for (session, stream) in self.index.consumer_journals.keys() {
            self.loaded
                .insert(ProducerMetadataKey::ConsumerHead(session.clone(), *stream));
        }
        for session in self.index.session_entity_parent_start_indices.keys() {
            self.loaded
                .insert(ProducerMetadataKey::Session(session.clone()));
            self.new_sessions.insert(session.clone());
        }
        // Empty old page tails too: appending beyond a shortened catalogue must not reload them.
        for sessions in self.index.session_pages.values_mut() {
            sessions.clear();
        }
        self.index.session_count = 0;
        Ok(ForkControlProjection {
            source_sessions,
            retained_streams: self.index.streams.keys().copied().collect(),
        })
    }

    async fn catalogue_new_sessions(&mut self) -> Result<(), String> {
        for session in std::mem::take(&mut self.new_sessions) {
            if !self
                .index
                .session_entity_parent_start_indices
                .contains_key(&session)
            {
                continue;
            }
            let page = self.index.session_count / SESSION_PAGE_SIZE;
            self.load(ProducerMetadataKey::SessionPage(page)).await?;
            self.index
                .session_pages
                .entry(page)
                .or_default()
                .push(session);
            self.index.session_count = self
                .index
                .session_count
                .checked_add(1)
                .ok_or("producer session catalogue overflow")?;
        }
        Ok(())
    }

    async fn catalogue_attachment(
        &mut self,
        attachment: AttachmentId,
        stream: StreamId,
        environment: EnvironmentId,
        consumer: AgentId,
    ) -> Result<(), String> {
        let key = (attachment, stream, environment, consumer.clone());
        self.load(ProducerMetadataKey::AttachmentPosition(
            attachment,
            stream,
            environment,
            consumer,
        ))
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
                    .push(key.clone());
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
                    self.load(ProducerMetadataKey::AttachmentPosition(
                        moved.0,
                        moved.1,
                        moved.2,
                        moved.3.clone(),
                    ))
                    .await?;
                    *self
                        .index
                        .attachment_pages
                        .get_mut(&(position / ATTACHMENT_PAGE_SIZE))
                        .and_then(|page| page.get_mut((position % ATTACHMENT_PAGE_SIZE) as usize))
                        .ok_or("active attachment catalogue slot is missing")? = moved.clone();
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

    async fn registration(&mut self, record: &StreamRegisteredRecord) -> Result<(), String> {
        if let StreamRegistrationCoordinate::Nested {
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

    async fn session(&mut self, record: &StreamSessionRecord) -> Result<(), String> {
        if let Some(session) = crate::worker::stream_session_record_key(record) {
            self.load(ProducerMetadataKey::Session(session.clone()))
                .await?;
        }
        let mappings = match record {
            StreamSessionRecord::Prepared(record) => record.stream_mappings.as_slice(),
            StreamSessionRecord::InvocationResult(record) => record.stream_mappings.as_slice(),
            StreamSessionRecord::Mapping(record) => std::slice::from_ref(&record.mapping),
            _ => &[],
        };
        for mapping in mappings {
            self.load(ProducerMetadataKey::Reference(mapping.handle.stream_id))
                .await?;
        }
        let attachment = match record {
            StreamSessionRecord::AttachmentPrepared(record) => Some(&record.key),
            StreamSessionRecord::AttachmentActivated(record) => Some(&record.key),
            StreamSessionRecord::AttachmentRenewed(record) => Some(&record.key),
            StreamSessionRecord::AttachmentFinalized(record) => Some(&record.key),
            StreamSessionRecord::CascadeOutbox(record) => {
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
                key.consumer_environment_id,
                key.consumer.clone(),
            ))
            .await?;
            self.load(ProducerMetadataKey::ActiveAttachmentCount(
                key.session_key.clone(),
                key.stream_id,
            ))
            .await?;
        }
        let head = match record {
            StreamSessionRecord::ConsumerItemValue(record) => {
                Some((&record.session_key, record.stream_id))
            }
            StreamSessionRecord::ConsumerTerminal(record) => {
                Some((&record.session_key, record.stream_id))
            }
            StreamSessionRecord::SourceUnavailable(record) => {
                Some((&record.key.session_key, record.key.stream_id))
            }
            _ => None,
        };
        if let Some((session, stream)) = head {
            self.load(ProducerMetadataKey::ConsumerHead(session.clone(), stream))
                .await?;
        }
        if let StreamSessionRecord::ExternalProducerState(record) = record {
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

/// Rebuilds metadata rows from committed producer history without changing that history.
pub(crate) async fn project_producer_metadata(
    storage: &(dyn KeyValueStorage + Send + Sync),
    namespace: KeyValueStorageNamespace,
    oplog: &dyn OplogService,
    owner: &OwnedAgentId,
    mode: AgentMode,
    fingerprint: AgentFingerprint,
    entries: &BTreeMap<OplogIndex, OplogEntry>,
    lineage: &StreamForkLineage,
) -> Result<ProducerMetadataProjection, String> {
    let mut projection = Projection {
        index: ProducerStreamIndex::default(),
        loaded: HashSet::new(),
        new_sessions: HashSet::new(),
        replacements: HashMap::new(),
        storage,
        namespace,
    };
    projection.load(ProducerMetadataKey::Global).await?;
    let mut pending = Vec::new();
    let mut fork = None;
    for (index, entry) in entries {
        let (author, fingerprint) = lineage.author_at(*index, owner, fingerprint);
        if !pending.is_empty()
            && !matches!(
                entry,
                OplogEntry::StreamRegistered { .. } | OplogEntry::StreamItems { .. }
            )
        {
            return Err("nested registration batch is missing its enclosing item".into());
        }
        match entry {
            OplogEntry::StreamRegistered {
                entity_parent_start_index,
                record,
                ..
            } => {
                let record = oplog.download_payload(owner, mode, record.clone()).await?;
                projection.registration(&record).await?;
                if matches!(
                    record.coordinate,
                    StreamRegistrationCoordinate::Nested { .. }
                ) {
                    pending.push((*index, *entity_parent_start_index, record));
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
                            *entity_parent_start_index,
                            record,
                            author.environment_id,
                            &author.agent_id,
                            fingerprint,
                        )
                        .map_err(|error| error.to_string())?;
                }
            }
            OplogEntry::StreamItems {
                entity_parent_start_index,
                record,
                ..
            } => {
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
                        *entity_parent_start_index,
                        std::mem::take(&mut pending),
                        record,
                        author.environment_id,
                        &author.agent_id,
                        fingerprint,
                    )
                    .map_err(|error| error.to_string())?;
            }
            OplogEntry::StreamEnd {
                entity_parent_start_index,
                record,
                ..
            } => {
                let record = oplog.download_payload(owner, mode, record.clone()).await?;
                projection.stream(record.stream_id).await?;
                projection
                    .index
                    .apply_end(*index, *entity_parent_start_index, record, fingerprint)
                    .map_err(|error| error.to_string())?;
            }
            OplogEntry::StreamCancel {
                entity_parent_start_index,
                record,
                ..
            } => {
                let record = oplog.download_payload(owner, mode, record.clone()).await?;
                projection.stream(record.stream_id).await?;
                projection
                    .index
                    .apply_cancel(*index, *entity_parent_start_index, record, fingerprint)
                    .map_err(|error| error.to_string())?;
            }
            OplogEntry::StreamSession {
                entity_parent_start_index,
                record,
                ..
            } => {
                let record = oplog.download_payload(owner, mode, record.clone()).await?;
                if let StreamSessionRecord::ForkCut(cut) = &record {
                    if entries.len() != 1 {
                        return Err("fork marker must occupy its own projection transaction".into());
                    }
                    if lineage
                        .cuts()
                        .iter()
                        .find(|(position, _)| position == index)
                        .map(|(_, record)| record)
                        != Some(cut)
                    {
                        return Err("fork marker differs from the validated lineage".into());
                    }
                    fork = Some(projection.fork_cut(cut).await?);
                    continue;
                }
                projection.session(&record).await?;
                projection
                    .index
                    .apply_session_references(*entity_parent_start_index, &record)
                    .map_err(|error| error.to_string())?;
                projection.index.apply_result_offset(*index, &record);
                if let StreamSessionRecord::ExternalProducerState(value) = &record {
                    projection.index.apply_external_producer_state(value);
                }
                projection
                    .index
                    .apply_deletion_record(
                        &record,
                        author.environment_id,
                        &author.agent_id,
                        fingerprint,
                    )
                    .map_err(|error| error.to_string())?;
                projection
                    .index
                    .apply_attachment_record(
                        &record,
                        author.environment_id,
                        &author.agent_id,
                        fingerprint,
                    )
                    .map_err(|error| error.to_string())?;
                for key in ProducerMetadataKey::session_record(&record) {
                    if let ProducerMetadataKey::Attachment(
                        attachment,
                        stream,
                        environment,
                        consumer,
                    ) = key
                    {
                        projection
                            .catalogue_attachment(attachment, stream, environment, consumer)
                            .await?;
                    }
                }
                if let StreamSessionRecord::Finished(record) = &record {
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
    projection.catalogue_new_sessions().await?;
    let rows = projection
        .loaded
        .into_iter()
        .filter_map(|key| {
            projection
                .replacements
                .remove(&key)
                .or_else(|| projection.index.metadata_row(&key))
                .map(|row| (key, row))
        })
        .map(|(key, row)| Ok((key.field()?, serialize(&row)?)))
        .collect::<Result<_, String>>()?;
    Ok(ProducerMetadataProjection { rows, fork })
}

impl DurableStreamStore {
    /// Loads a producer index from durable metadata, falling back to committed oplog records.
    pub(crate) async fn load_indexed_with_commit(
        oplog: Arc<dyn Oplog>,
        owner: OwnedAgentId,
        producer_fingerprint: AgentFingerprint,
        live_join_capacity: Option<usize>,
        commit: DurableStreamCommit,
        service: Arc<dyn WorkerService>,
        mode: AgentMode,
    ) -> Result<Arc<Self>, StreamStoreError> {
        let live_join_capacity = live_join_capacity.unwrap_or(DEFAULT_LIVE_JOIN_BUFFER_SIZE);
        DurableLiveStreamBus::<CommittedProducerStreamEvent>::new(live_join_capacity)?;
        let (_, mut rows) = service
            .lookup_durable_stream_producer_metadata(
                &owner,
                mode,
                vec![ProducerMetadataKey::Global, ProducerMetadataKey::Lineage],
            )
            .await
            .map_err(StreamStoreError::Oplog)?;
        if rows.len() != 2 {
            return Err(StreamStoreError::CorruptHistory(
                "producer metadata lookup returned an incorrect row count".into(),
            ));
        }
        let mut index = ProducerStreamIndex::default();
        let Some(ProducerMetadataRow::Lineage(lineage)) = rows.pop().flatten() else {
            return Err(StreamStoreError::CorruptHistory(
                "producer metadata lookup did not return fork lineage".into(),
            ));
        };
        index.fork_lineage = lineage;
        if let Some(row) = rows.pop().flatten() {
            index
                .hydrate_metadata_row(ProducerMetadataKey::Global, row)
                .map_err(StreamStoreError::CorruptHistory)?;
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
    ) -> Result<MutexGuard<'_, ProducerStreamIndex>, StreamStoreError> {
        self.index_for_query(keys.into_iter().collect(), None, None)
            .await
    }

    pub(super) async fn index_for_terminal(
        &self,
        keys: impl IntoIterator<Item = ProducerMetadataKey> + Send,
        stream: StreamId,
    ) -> Result<MutexGuard<'_, ProducerStreamIndex>, StreamStoreError> {
        let mut keys = keys.into_iter().collect::<Vec<_>>();
        keys.push(ProducerMetadataKey::Stream(stream));
        self.index_for_query(keys, Some(stream), None).await
    }

    pub(super) async fn index_for_session_streams(
        &self,
        session: &StreamSessionKey,
    ) -> Result<MutexGuard<'_, ProducerStreamIndex>, StreamStoreError> {
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
        session: Option<&StreamSessionKey>,
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
                | ProducerMetadataKey::Attachment(_, stream, _, _) => {
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
                ProducerMetadataKey::ExternalProducerSequence(
                    session,
                    stream,
                    producer,
                    epoch,
                    sequence,
                ) => {
                    index.external_producer_offsets.contains_key(&(
                        session.clone(),
                        *stream,
                        producer.clone(),
                        *epoch,
                        *sequence,
                    )) || index
                        .external_producer_heads
                        .get(&(session.clone(), *stream, producer.clone()))
                        .is_some_and(|head| {
                            *epoch > head.epoch
                                || *epoch == head.epoch && *sequence >= head.next_sequence
                        })
                        || index.loaded_metadata.contains(
                            &ProducerMetadataKey::ExternalProducerHead(
                                session.clone(),
                                *stream,
                                producer.clone(),
                            ),
                        ) && !index.external_producer_heads.contains_key(&(
                            session.clone(),
                            *stream,
                            producer.clone(),
                        ))
                }
                _ => false,
            }
    }

    #[tracing::instrument(name = "durable_stream.metadata.query", level = "debug", skip_all)]
    async fn index_for_query(
        &self,
        keys: Vec<ProducerMetadataKey>,
        terminal: Option<StreamId>,
        session: Option<StreamSessionKey>,
    ) -> Result<MutexGuard<'_, ProducerStreamIndex>, StreamStoreError> {
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
            // Never return a guard in the task result: a suspended store may stop polling
            // its JoinHandle indefinitely. The task owns and releases all hydration locks.
            self.tasks()
                .spawn_metadata(producer.hydrate_query(keys.clone(), terminal, session.clone()))
                .map_err(|error| StreamStoreError::Oplog(error.to_string()))?
                .await
                .map_err(|error| StreamStoreError::Oplog(error.to_string()))??;
        }
    }

    async fn hydrate_query(
        self: Arc<Self>,
        keys: Vec<ProducerMetadataKey>,
        terminal: Option<StreamId>,
        session: Option<StreamSessionKey>,
    ) -> Result<(), StreamStoreError> {
        let mut index = self.index.lock().await;
        if !index.complete_for_deletion
            && self.control_metadata_provider.get().is_some()
            && (index.loaded_metadata.len() > 128
                || index.batch_positions.len() > 128
                || index.streams.len() > 128)
        {
            *index = ProducerStreamIndex::default();
            self.buses
                .write()
                .expect("durable stream bus map lock poisoned")
                .retain(|_, bus| Arc::strong_count(bus) > 1 || bus.has_pending_deferred_terminal());
        }
        self.load_index_keys(&mut index, keys.clone()).await?;
        let required = self.query_keys(&index, &keys, session.as_ref());
        self.load_index_keys(&mut index, required).await?;
        if let Some(stream) = terminal {
            self.load_terminal(&mut index, stream).await?;
        }
        Ok(())
    }

    pub(super) async fn index_for_cleanup(
        &self,
    ) -> Result<MutexGuard<'_, ProducerStreamIndex>, StreamStoreError> {
        let mut index = self.index.lock().await;
        self.ensure_healthy()?;
        if self.control_metadata_provider.get().is_some() && !index.complete_for_deletion {
            let mut complete = Self::read_complete_index(
                self.oplog.as_ref(),
                self.environment_id,
                &self.producer,
                self.producer_fingerprint,
                self.oplog.current_oplog_index().await,
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
    ) -> Result<(), StreamStoreError> {
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
    ) -> Result<(), StreamStoreError> {
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
                    | ProducerMetadataKey::Attachment(_, stream, _, _) => {
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
                    ProducerMetadataKey::ExternalProducerSequence(
                        session,
                        stream,
                        producer,
                        _,
                        _,
                    ) => Some(ProducerMetadataKey::ExternalProducerHead(
                        session.clone(),
                        *stream,
                        producer.clone(),
                    )),
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
                .map_err(StreamStoreError::Oplog)?;
            if rows.len() != batch.len() {
                return Err(StreamStoreError::CorruptHistory(
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
                        .map_err(StreamStoreError::CorruptHistory)?;
                }
                index.loaded_metadata.insert(key);
            }
        }
        Ok(())
    }

    pub(super) async fn indexed_attachment_candidates(
        &self,
        batch_size: usize,
    ) -> Result<Option<IndexedAttachmentCandidateBatch>, StreamStoreError> {
        let producer = self
            .self_weak
            .upgrade()
            .expect("live producer has an owning Arc");
        self.tasks()
            .spawn_metadata(async move {
                producer
                    .indexed_attachment_candidates_inner(batch_size)
                    .await
            })
            .map_err(|error| StreamStoreError::Oplog(error.to_string()))?
            .await
            .map_err(|error| StreamStoreError::Oplog(error.to_string()))?
    }

    async fn indexed_attachment_candidates_inner(
        &self,
        batch_size: usize,
    ) -> Result<Option<IndexedAttachmentCandidateBatch>, StreamStoreError> {
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
            .map_err(StreamStoreError::Oplog)?;
        let Some(ProducerMetadataRow::Global {
            active_attachments: count,
            deleting,
            ..
        }) = rows.pop().flatten()
        else {
            return Ok(Some(IndexedAttachmentCandidateBatch {
                deleting: false,
                candidates: Vec::new(),
            }));
        };
        if count == 0 || batch_size == 0 {
            return Ok(Some(IndexedAttachmentCandidateBatch {
                deleting,
                candidates: Vec::new(),
            }));
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
            .map_err(StreamStoreError::Oplog)?;
        let mut catalogue = HashMap::new();
        for (page, row) in pages.into_iter().zip(rows) {
            let Some(ProducerMetadataRow::AttachmentPage(slots)) = row else {
                return Err(StreamStoreError::CorruptHistory(
                    "active attachment catalogue page is missing".into(),
                ));
            };
            catalogue.insert(page, slots);
        }
        let mut keys = Vec::new();
        for position in positions {
            let (attachment, stream, environment, consumer) = catalogue
                .get(&(position / ATTACHMENT_PAGE_SIZE))
                .and_then(|page| page.get((position % ATTACHMENT_PAGE_SIZE) as usize))
                .ok_or_else(|| {
                    StreamStoreError::CorruptHistory(
                        "active attachment catalogue slot is missing".into(),
                    )
                })?;
            keys.push(ProducerMetadataKey::Attachment(
                *attachment,
                *stream,
                *environment,
                consumer.clone(),
            ));
            keys.push(ProducerMetadataKey::Stream(*stream));
        }
        let (_, rows) = service
            .lookup_durable_stream_producer_metadata(&owner, *mode, keys)
            .await
            .map_err(StreamStoreError::Oplog)?;
        let mut candidates = Vec::new();
        for pair in rows.as_chunks::<2>().0 {
            let [
                Some(ProducerMetadataRow::Attachment(attachment)),
                Some(ProducerMetadataRow::Stream(stream)),
            ] = pair
            else {
                return Err(StreamStoreError::CorruptHistory(
                    "active attachment catalogue refers to missing metadata".into(),
                ));
            };
            candidates.push(IndexedAttachmentCandidate {
                attachment: attachment.clone(),
                journal_summary: ProducerJournalSummary {
                    event_count: stream.next_sequence + u64::from(stream.terminal),
                    last_offset: stream.last_offset,
                    terminal: stream.terminal,
                },
            });
        }
        Ok(Some(IndexedAttachmentCandidateBatch {
            deleting,
            candidates,
        }))
    }

    /// Counts committed source events beyond the consumer's last journaled offset.
    pub async fn journal_lag_events(
        &self,
        handle: &DurableStreamHandle,
        after: Option<StreamOffset>,
    ) -> Result<usize, StreamStoreError> {
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
                return Err(StreamStoreError::InvalidHandle);
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
                StreamStoreError::Oplog("producer metadata service is unavailable".into())
            })?;
            let owner = OwnedAgentId::new(handle.producer_environment_id, &handle.producer);
            let mode = service
                .get_agent_mode(&owner)
                .await
                .map_err(|err| StreamStoreError::Oplog(err.to_string()))?
                .ok_or(StreamStoreError::InvalidHandle)?;
            let (_, rows) = service
                .lookup_durable_stream_producer_metadata(&owner, mode, keys)
                .await
                .map_err(StreamStoreError::Oplog)?;
            let mut rows = rows.into_iter();
            let Some(Some(ProducerMetadataRow::Stream(stream))) = rows.next() else {
                return Err(StreamStoreError::InvalidHandle);
            };
            let position = match rows.next().flatten() {
                Some(ProducerMetadataRow::Position(first, count)) => Some((first, count)),
                _ => None,
            };
            (stream, position)
        };
        if stream.registration.handle != *handle {
            return Err(StreamStoreError::InvalidHandle);
        }
        let total = stream
            .next_sequence
            .checked_add(u64::from(stream.terminal))
            .ok_or(StreamStoreError::CounterOverflow)?;
        let consumed = match after {
            None => 0,
            Some(after) if stream.terminal && stream.last_offset == Some(after) => total,
            Some(after) => {
                let (first, count) = position.ok_or(StreamStoreError::CursorUnavailable)?;
                if u64::from(after.sub_index()) >= count
                    || stream.last_offset.is_none_or(|last| after > last)
                {
                    return Err(StreamStoreError::CursorUnavailable);
                }
                first
                    .checked_add(u64::from(after.sub_index()) + 1)
                    .ok_or(StreamStoreError::CounterOverflow)?
            }
        };
        usize::try_from(
            total
                .checked_sub(consumed)
                .ok_or(StreamStoreError::CursorUnavailable)?,
        )
        .map_err(|_| StreamStoreError::CounterOverflow)
    }

    pub(super) async fn load_terminal(
        &self,
        index: &mut ProducerStreamIndex,
        stream_id: StreamId,
    ) -> Result<(), StreamStoreError> {
        for (id, stream) in &mut index.streams {
            if *id != stream_id {
                stream.terminal_event = None;
            }
        }
        let stream = index
            .streams
            .get_mut(&stream_id)
            .ok_or(StreamStoreError::UnknownStream(stream_id))?;
        if !stream.terminal || stream.terminal_event.is_some() {
            return Ok(());
        }
        let offset = stream.last_offset.ok_or_else(|| {
            StreamStoreError::CorruptHistory(
                "terminal stream metadata has no durable offset".into(),
            )
        })?;
        let event =
            read_terminal_event(self.oplog.as_ref(), &self.fork_lineage, stream_id, offset).await?;
        if event.producer_sequence != stream.next_sequence {
            return Err(StreamStoreError::CorruptHistory(
                "terminal metadata does not match its durable record".into(),
            ));
        }
        stream.terminal_event = Some(event);
        Ok(())
    }
}

pub(super) async fn read_terminal_event(
    oplog: &dyn Oplog,
    lineage: &StreamForkLineage,
    stream_id: StreamId,
    offset: StreamOffset,
) -> Result<CommittedProducerStreamEvent, StreamStoreError> {
    let (id, sequence, recorded_offset, author, payload) =
        match oplog.read(offset.producer_oplog_index()).await {
            OplogEntry::StreamEnd { record, .. } => {
                let record = oplog
                    .download_payload(record)
                    .await
                    .map_err(StreamStoreError::Oplog)?;
                (
                    record.stream_id,
                    record.sequence,
                    record.offset,
                    record.authored_by,
                    CommittedProducerStreamEventPayload::End(record.result),
                )
            }
            OplogEntry::StreamCancel { record, .. } => {
                let record = oplog
                    .download_payload(record)
                    .await
                    .map_err(StreamStoreError::Oplog)?;
                (
                    record.stream_id,
                    record.sequence,
                    record.offset,
                    record.authored_by,
                    CommittedProducerStreamEventPayload::Cancel {
                        role: record.role,
                        reason: record.reason,
                        details: record.details,
                    },
                )
            }
            _ => {
                return Err(StreamStoreError::CorruptHistory(
                    "terminal metadata points at a non-terminal record".into(),
                ));
            }
        };
    let mut id = id;
    for (_, cut) in lineage.cuts() {
        if offset.producer_oplog_index() <= cut.cut_index {
            if cut.selected_stream_id == Some(id) {
                return Err(StreamStoreError::CorruptHistory(
                    "selected stream terminal is outside its retained prefix".into(),
                ));
            }
            if let Some(mapping) = cut
                .streams
                .iter()
                .find(|mapping| mapping.source.stream_id == id)
            {
                id = mapping.continuation.stream_id;
            }
        }
    }
    if id != stream_id || recorded_offset != offset {
        return Err(StreamStoreError::CorruptHistory(
            "terminal metadata does not match its durable record".into(),
        ));
    }
    Ok(CommittedProducerStreamEvent {
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
    use crate::storage::keyvalue::fault_injecting::{
        FaultInjectingKeyValueStorage, KeyValueStorageFaults,
    };
    use crate::storage::keyvalue::memory::InMemoryKeyValueStorage;
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::durable_stream::StreamRootKind;
    use golem_common::model::{AgentMetadata, AgentStatusRecord, RetryConfig, Timestamp};
    use golem_common::read_only_lock;
    use test_r::{test, timeout};

    struct AliasFixture {
        storage: InMemoryKeyValueStorage,
        namespace: KeyValueStorageNamespace,
    }

    impl AliasFixture {
        fn new() -> Self {
            Self {
                storage: InMemoryKeyValueStorage::new(),
                namespace: KeyValueStorageNamespace::AgentDurableStreamSessionIndex {
                    agent_id: identity().agent_id,
                },
            }
        }

        async fn put(&self, key: ProducerMetadataKey, row: ProducerMetadataRow) {
            self.storage
                .with_entity("test", "prefix_alias", "metadata")
                .set(self.namespace.clone(), &key.field().unwrap(), &row)
                .await
                .unwrap();
        }

        async fn get(
            &self,
            key: ProducerMetadataKey,
        ) -> Result<Option<ProducerMetadataRow>, String> {
            let row = self
                .storage
                .with_entity("test", "prefix_alias", "metadata")
                .get(self.namespace.clone(), &key.field()?)
                .await?;
            resolve_producer_locator(
                &self.storage,
                self.namespace.clone(),
                &key,
                row,
                &mut HashMap::new(),
            )
            .await
        }

        async fn alias(&self, stream: StreamId, previous: StreamId, retained: u64) {
            self.put(
                ProducerMetadataKey::PrefixAlias(stream),
                ProducerMetadataRow::PrefixAlias {
                    previous_stream_id: previous,
                    retained_next_sequence: retained,
                },
            )
            .await;
        }
    }

    #[test]
    async fn prefix_alias_chain_preserves_intermediate_appends_and_clips_positions() {
        let fixture = AliasFixture::new();
        let source = StreamId(uuid::Uuid::from_u128(1));
        let first = StreamId(uuid::Uuid::from_u128(2));
        let second = StreamId(uuid::Uuid::from_u128(3));
        let old_batch = OplogIndex::from_u64(20);
        let appended_batch = OplogIndex::from_u64(40);
        fixture.alias(first, source, 5).await;
        fixture.alias(second, first, 8).await;
        for (stream, sequence, count, offset) in [
            (source, 2, 8, old_batch),
            (source, 10, 5, OplogIndex::from_u64(21)),
            (first, 5, 6, appended_batch),
        ] {
            fixture
                .put(
                    ProducerMetadataKey::Batch(stream, sequence),
                    ProducerMetadataRow::Batch(offset),
                )
                .await;
            fixture
                .put(
                    ProducerMetadataKey::Position(stream, offset),
                    ProducerMetadataRow::Position(sequence, count),
                )
                .await;
        }
        assert!(
            matches!(fixture.get(ProducerMetadataKey::Batch(second, 2)).await.unwrap(), Some(ProducerMetadataRow::Batch(offset)) if offset == old_batch)
        );
        assert!(matches!(
            fixture
                .get(ProducerMetadataKey::Position(second, old_batch))
                .await
                .unwrap(),
            Some(ProducerMetadataRow::Position(2, 3))
        ));
        assert!(
            matches!(fixture.get(ProducerMetadataKey::Batch(second, 5)).await.unwrap(), Some(ProducerMetadataRow::Batch(offset)) if offset == appended_batch)
        );
        assert!(matches!(
            fixture
                .get(ProducerMetadataKey::Position(second, appended_batch))
                .await
                .unwrap(),
            Some(ProducerMetadataRow::Position(5, 3))
        ));
        assert!(
            fixture
                .get(ProducerMetadataKey::Position(
                    second,
                    OplogIndex::from_u64(21)
                ))
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            fixture
                .get(ProducerMetadataKey::Batch(second, 8))
                .await
                .unwrap()
                .is_none()
        );

        fixture
            .put(
                ProducerMetadataKey::Batch(first, 8),
                ProducerMetadataRow::Batch(appended_batch),
            )
            .await;
        assert!(
            fixture
                .get(ProducerMetadataKey::Batch(second, 8))
                .await
                .unwrap()
                .is_none()
        );
        let own_batch = OplogIndex::from_u64(60);
        fixture
            .put(
                ProducerMetadataKey::Batch(second, 8),
                ProducerMetadataRow::Batch(own_batch),
            )
            .await;
        fixture
            .put(
                ProducerMetadataKey::Position(second, own_batch),
                ProducerMetadataRow::Position(8, 4),
            )
            .await;
        assert!(
            matches!(fixture.get(ProducerMetadataKey::Batch(second, 8)).await.unwrap(), Some(ProducerMetadataRow::Batch(offset)) if offset == own_batch)
        );
        assert!(matches!(
            fixture
                .get(ProducerMetadataKey::Position(second, own_batch))
                .await
                .unwrap(),
            Some(ProducerMetadataRow::Position(8, 4))
        ));

        let shorter = StreamId(uuid::Uuid::from_u128(4));
        fixture.alias(shorter, first, 3).await;
        assert!(matches!(
            fixture
                .get(ProducerMetadataKey::Position(shorter, old_batch))
                .await
                .unwrap(),
            Some(ProducerMetadataRow::Position(2, 1))
        ));
    }

    #[test]
    async fn prefix_alias_never_resolves_authority_or_an_empty_prefix() {
        let fixture = AliasFixture::new();
        let source = StreamId(uuid::Uuid::from_u128(1));
        let target = StreamId(uuid::Uuid::from_u128(2));
        let offset = OplogIndex::from_u64(10);
        fixture.alias(target, source, 0).await;
        fixture
            .put(
                ProducerMetadataKey::Batch(source, 0),
                ProducerMetadataRow::Batch(offset),
            )
            .await;
        fixture
            .put(
                ProducerMetadataKey::Position(source, offset),
                ProducerMetadataRow::Position(0, 7),
            )
            .await;
        assert!(
            fixture
                .get(ProducerMetadataKey::Batch(target, 0))
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            fixture
                .get(ProducerMetadataKey::Position(target, offset))
                .await
                .unwrap()
                .is_none()
        );

        fixture.alias(target, source, 7).await;
        // Deliberately wrong authority rows detect accidental traversal without constructing
        // a registration: only locator lookups may ever read these source keys.
        fixture
            .put(
                ProducerMetadataKey::Stream(source),
                ProducerMetadataRow::Batch(offset),
            )
            .await;
        fixture
            .put(
                ProducerMetadataKey::Reference(source),
                ProducerMetadataRow::Batch(offset),
            )
            .await;
        assert!(
            fixture
                .get(ProducerMetadataKey::Stream(target))
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            fixture
                .get(ProducerMetadataKey::Reference(target))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[test]
    async fn prefix_alias_rejects_cycles_and_invalid_locator_rows() {
        let fixture = AliasFixture::new();
        let source = StreamId(uuid::Uuid::from_u128(1));
        let target = StreamId(uuid::Uuid::from_u128(2));
        fixture.alias(target, source, 5).await;
        fixture.alias(source, target, 9).await;
        assert!(
            fixture
                .get(ProducerMetadataKey::Batch(target, 2))
                .await
                .err()
                .unwrap()
                .contains("cycle")
        );
        fixture
            .put(
                ProducerMetadataKey::Batch(source, 2),
                ProducerMetadataRow::Position(2, 3),
            )
            .await;
        assert!(
            fixture
                .get(ProducerMetadataKey::Batch(target, 2))
                .await
                .err()
                .unwrap()
                .contains("row type")
        );
        let offset = OplogIndex::from_u64(10);
        fixture
            .put(
                ProducerMetadataKey::Position(source, offset),
                ProducerMetadataRow::Position(u64::MAX, 2),
            )
            .await;
        assert!(
            fixture
                .get(ProducerMetadataKey::Position(target, offset))
                .await
                .err()
                .unwrap()
                .contains("overflow")
        );
        fixture
            .put(
                ProducerMetadataKey::PrefixAlias(target),
                ProducerMetadataRow::Batch(offset),
            )
            .await;
        assert!(
            fixture
                .get(ProducerMetadataKey::Batch(target, 3))
                .await
                .err()
                .unwrap()
                .contains("row type")
        );
    }

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
            Self::with_storage(archive, Arc::new(InMemoryKeyValueStorage::new())).await
        }

        async fn with_storage(
            archive: bool,
            storage: Arc<dyn KeyValueStorage + Send + Sync>,
        ) -> Self {
            Self::with_buffer(archive, storage, 1).await
        }

        async fn with_buffer(
            archive: bool,
            storage: Arc<dyn KeyValueStorage + Send + Sync>,
            max_operations_before_commit: u64,
        ) -> Self {
            let identity = identity();
            let owner = OwnedAgentId::new(identity.environment_id, &identity.agent_id);
            let indexed = Arc::new(ReadCountingIndexedStorage::new());
            let blobs = Arc::new(ReadCountingBlobStorage::new());
            let primary = Arc::new(
                PrimaryOplogService::new(
                    indexed.clone(),
                    blobs.clone(),
                    max_operations_before_commit,
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
            let service = Arc::new(DefaultWorkerService::new(
                storage,
                Arc::new(ShardServiceDefault::new()),
                primary.clone(),
                Arc::new(UnusedComponentService),
                Arc::new(GolemConfig::default()),
            ));
            let oplog = primary
                .create_fresh(
                    &mut primary.lock_lifecycle(&owner.agent_id).await,
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
            Self {
                identity,
                service,
                oplog,
                indexed,
                blobs,
            }
        }

        async fn producer(&self) -> Arc<DurableStreamStore> {
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
            DurableStreamStore::load_indexed_with_commit(
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

        fn registration(&self, ordinal: u32) -> ProducerRegistrationRequest {
            registration(
                &self.identity,
                StreamRegistrationCoordinate::Root {
                    invocation_id: self.identity.invocation.clone(),
                    root_kind: StreamRootKind::MethodResult,
                    recursive_value_path: vec![
                        golem_common::model::durable_stream::StreamValuePathStep::TupleElement(
                            ordinal,
                        ),
                    ],
                },
                StreamSourceKind::InvocationOutput,
            )
        }

        fn input_registration(&self, ordinal: u32) -> ProducerRegistrationRequest {
            registration(
                &self.identity,
                StreamRegistrationCoordinate::Root {
                    invocation_id: self.identity.invocation.clone(),
                    root_kind: StreamRootKind::MethodInput,
                    recursive_value_path: vec![
                        golem_common::model::durable_stream::StreamValuePathStep::TupleElement(
                            ordinal,
                        ),
                    ],
                },
                StreamSourceKind::ExternalInlineInput,
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
    #[timeout("30s")]
    async fn active_attachment_count_is_available_after_cold_metadata_load() {
        let fixture = Fixture::new().await;
        let producer = fixture.producer().await;
        let handle = producer
            .register(None, fixture.registration(0))
            .await
            .unwrap()
            .value;
        let mut key = crate::durable_host::durable_stream::tests::attachment_key(
            &fixture.identity,
            handle.stream_id,
        );
        key.session_key = fixture.identity.invocation.clone();
        key.attachment_id = AttachmentId::primary(
            key.session_key.callee_environment_id,
            &key.session_key.callee,
            &key.session_key.idempotency_key,
        )
        .unwrap();
        producer.prepare_attachment(key.clone(), 100).await.unwrap();
        producer
            .activate_attachment(key.clone(), 110)
            .await
            .unwrap();
        fixture.persist().await;
        drop(producer);

        let cold = fixture.producer().await;
        assert!(
            cold.has_active_attachment(&key.session_key, &handle)
                .await
                .unwrap()
        );
        assert!(cold.index.lock().await.attachments.is_empty());
        key.epoch += 1;
        cold.prepare_attachment(key.clone(), 120).await.unwrap();
        assert!(
            !cold
                .has_active_attachment(&key.session_key, &handle)
                .await
                .unwrap()
        );
        cold.activate_attachment(key.clone(), 130).await.unwrap();
        assert!(
            cold.activate_attachment(key.clone(), 130)
                .await
                .unwrap()
                .replayed
        );
        fixture.persist().await;
        drop(cold);

        let cold = fixture.producer().await;
        cold.finalize_attachment(
            key.clone(),
            StreamAttachmentFinalizationReason::ConsumerFinalized,
            140,
        )
        .await
        .unwrap();
        assert!(
            !cold
                .has_active_attachment(&key.session_key, &handle)
                .await
                .unwrap()
        );
        fixture.persist().await;
        drop(cold);
        assert!(
            !fixture
                .producer()
                .await
                .has_active_attachment(&key.session_key, &handle)
                .await
                .unwrap()
        );
    }

    #[test]
    #[timeout("60s")]
    async fn persisted_metadata_restores_entity_attribution() {
        let fixture = Fixture::new().await;
        let producer = fixture.producer().await;
        let entity_parent_start_index = Some(OplogIndex::from_u64(42));
        let mut request = fixture.registration(0);
        request.entity_parent_start_index = entity_parent_start_index;
        let handle = producer.register(None, request).await.unwrap().value;
        fixture.persist().await;
        drop(producer);

        let cold = fixture.producer().await;
        let index = cold
            .index_for([
                ProducerMetadataKey::Stream(handle.stream_id),
                ProducerMetadataKey::Session(fixture.identity.invocation.clone()),
            ])
            .await
            .unwrap();
        assert_eq!(
            index.entity_parent_start_index(handle.stream_id).unwrap(),
            entity_parent_start_index
        );
        assert_eq!(
            index.session_entity_parent_start_index(&fixture.identity.invocation),
            entity_parent_start_index
        );
        drop(index);
        cold.register_result_streams(
            None,
            fixture.identity.invocation.clone(),
            vec![17],
            vec![ProducerOutputRegistration {
                transport_stream_id: 0,
                source: ProducerOutputSource::Existing(handle),
                cancellation_epoch: None,
            }],
            entity_parent_start_index,
        )
        .await
        .unwrap();
        fixture.persist().await;
        drop(cold);

        fixture
            .producer()
            .await
            .finish_session(
                None,
                fixture.identity.invocation.clone(),
                entity_parent_start_index,
                Ok(()),
                StreamCancelReason::Protocol,
            )
            .await
            .unwrap();
        let tip = fixture.oplog.current_oplog_index().await;
        for position in 2..=tip.as_u64() {
            let entry = fixture.oplog.read(OplogIndex::from_u64(position)).await;
            assert_eq!(entry.entity_parent_start_index(), entity_parent_start_index);
        }
        assert!(matches!(
            fixture.oplog.read(tip).await,
            OplogEntry::StreamSession { .. }
        ));
    }

    #[test]
    #[timeout("60s")]
    async fn result_without_owned_outputs_retains_entity_attribution_after_reload() {
        let fixture = Fixture::new().await;
        let attribution = Some(OplogIndex::from_u64(73));
        fixture
            .producer()
            .await
            .register_result_streams(
                None,
                fixture.identity.invocation.clone(),
                vec![29],
                Vec::new(),
                attribution,
            )
            .await
            .unwrap();
        let result_index = fixture.oplog.current_oplog_index().await;
        assert_eq!(
            fixture
                .oplog
                .read(result_index)
                .await
                .entity_parent_start_index(),
            attribution
        );
        fixture.persist().await;
        fixture
            .producer()
            .await
            .finish_session(
                None,
                fixture.identity.invocation.clone(),
                attribution,
                Ok(()),
                StreamCancelReason::Protocol,
            )
            .await
            .unwrap();
        let finished_index = fixture.oplog.current_oplog_index().await;
        assert_eq!(finished_index, result_index.next());
        let entry = fixture.oplog.read(finished_index).await;
        assert_eq!(entry.entity_parent_start_index(), attribution);
        let OplogEntry::StreamSession { record, .. } = entry else {
            panic!("expected session completion")
        };
        assert!(matches!(
            fixture.oplog.download_payload(record).await.unwrap(),
            StreamSessionRecord::Finished(_)
        ));
    }

    #[test]
    #[timeout("60s")]
    async fn external_producer_offsets_survive_indexed_cold_load() {
        let fixture = Fixture::new().await;
        let producer = fixture.producer().await;
        let handle = producer
            .register(None, fixture.input_registration(0))
            .await
            .unwrap()
            .value;
        let request = ExternalProducer {
            id: ExternalProducerId::Client("indexed".into()),
            epoch: 3,
            sequence: 0,
        };
        let accepted = producer
            .append_external_input(
                None,
                &fixture.identity.invocation,
                handle.stream_id,
                Some(StreamItemsPayload::PackedU8(vec![7])),
                false,
                Some(request.clone()),
            )
            .await
            .unwrap();
        let ExternalAppendOutcome::Accepted(offset) = accepted else {
            panic!()
        };
        fixture.persist().await;
        drop(producer);

        let cold = fixture.producer().await;
        assert_eq!(
            cold.append_external_input(
                None,
                &fixture.identity.invocation,
                handle.stream_id,
                Some(StreamItemsPayload::PackedU8(vec![7])),
                false,
                Some(request),
            )
            .await
            .unwrap(),
            ExternalAppendOutcome::Duplicate {
                offset,
                highest_sequence: Some(0),
            }
        );
    }

    #[test]
    #[timeout("60s")]
    async fn attached_transport_sequences_survive_indexed_cold_load_with_http_interleaving() {
        let fixture = Fixture::new().await;
        let producer = fixture.producer().await;
        let handle = producer
            .register(None, fixture.input_registration(97))
            .await
            .unwrap()
            .value;
        let first = producer
            .write_attached_items_with_nested(
                None,
                &fixture.identity.invocation,
                handle.stream_id,
                0,
                StreamItemsPayload::PackedU8(vec![10, 11]),
                Vec::new(),
            )
            .await
            .unwrap();
        assert_eq!(first.value.len(), 2);
        assert!(matches!(
            producer
                .append_external_input(
                    None,
                    &fixture.identity.invocation,
                    handle.stream_id,
                    Some(StreamItemsPayload::Values(vec![
                        vec![20],
                        vec![30],
                        vec![31],
                    ])),
                    false,
                    None,
                )
                .await
                .unwrap(),
            ExternalAppendOutcome::Accepted(_)
        ));
        let nested = registration(
            &fixture.identity,
            StreamRegistrationCoordinate::Nested {
                parent_stream_id: handle.stream_id,
                parent_producer_sequence: 2,
                recursive_value_path: vec![
                    golem_common::model::durable_stream::StreamValuePathStep::TupleElement(0),
                ],
            },
            StreamSourceKind::Nested,
        );
        let payload = StreamItemsPayload::Values(vec![vec![40]]);
        let reads = fixture.indexed.reads();
        assert_eq!(
            producer
                .attached_global_sequence(&fixture.identity.invocation, handle.stream_id, 2)
                .await
                .unwrap(),
            5
        );
        assert_eq!(
            fixture.indexed.reads(),
            reads,
            "a hot attached sequence lookup must not project an absent sequence from storage"
        );
        let fresh = producer
            .write_attached_items_with_nested(
                None,
                &fixture.identity.invocation,
                handle.stream_id,
                2,
                payload.clone(),
                vec![nested.clone()],
            )
            .await
            .unwrap();
        assert!(!fresh.replayed);
        assert_eq!(
            producer
                .attached_global_sequence(&fixture.identity.invocation, handle.stream_id, 2)
                .await
                .unwrap(),
            5
        );
        fixture.persist().await;
        drop(producer);

        let cold = fixture.producer().await;
        let retry = cold
            .write_attached_items_with_nested(
                None,
                &fixture.identity.invocation,
                handle.stream_id,
                2,
                payload.clone(),
                vec![nested.clone()],
            )
            .await
            .unwrap();
        assert!(retry.replayed);
        assert_eq!(retry.value, fresh.value);
        assert_eq!(
            cold.attached_global_sequence(&fixture.identity.invocation, handle.stream_id, 2)
                .await
                .unwrap(),
            5
        );
        let high_water = cold
            .attached_input_high_water(&fixture.identity.invocation, handle.stream_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(high_water.highest_contiguous_sequence, 2);
        assert!(!high_water.terminal);

        *cold.index.lock().await = ProducerStreamIndex::default();
        fixture.indexed.reset();
        let rehydrated = cold
            .write_attached_items_with_nested(
                None,
                &fixture.identity.invocation,
                handle.stream_id,
                2,
                payload,
                vec![nested],
            )
            .await
            .unwrap();
        assert!(rehydrated.replayed);
        assert_eq!(rehydrated.value, fresh.value);
        cold.append_external_input(
            None,
            &fixture.identity.invocation,
            handle.stream_id,
            None,
            true,
            Some(ExternalProducer {
                id: ExternalProducerId::Attached,
                epoch: 0,
                sequence: 3,
            }),
        )
        .await
        .unwrap();
        let high_water = cold
            .attached_input_high_water(&fixture.identity.invocation, handle.stream_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(high_water.highest_contiguous_sequence, 3);
        assert!(high_water.terminal);
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
            .finish_session(
                None,
                session.clone(),
                None,
                Ok(()),
                StreamCancelReason::Protocol,
            )
            .await
            .unwrap();
        fixture.persist().await;
        drop(producer);

        let cold = fixture.producer().await;
        assert!(cold.index.lock().await.finished_sessions.is_empty());
        let tip = fixture.oplog.current_oplog_index().await;
        assert_eq!(
            cold.ensure_session_accepts_new_events(&session).await,
            Err(StreamStoreError::SessionFinished(session.clone()))
        );
        let reads = fixture.indexed.reads();
        for _ in 0..3 {
            assert_eq!(
                cold.ensure_session_accepts_new_events(&session).await,
                Err(StreamStoreError::SessionFinished(session.clone()))
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
            Err(StreamStoreError::SessionFinished(session))
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
            .register(None, fixture.registration(0))
            .await
            .unwrap()
            .value;
        let mut offsets = Vec::new();
        for sequence in 0..140 {
            let outcome = producer
                .write_items(
                    None,
                    handle.stream_id,
                    sequence,
                    StreamItemsPayload::Values(vec![vec![sequence as u8; 128]]),
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
                None,
                handle.stream_id,
                0,
                StreamItemsPayload::Values(vec![vec![0; 128]]),
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
            .register(None, fixture.registration(0))
            .await
            .unwrap()
            .value;
        let offsets = producer
            .write_items(
                None,
                handle.stream_id,
                0,
                StreamItemsPayload::PackedU8((0..5000).map(|index| index as u8).collect()),
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
            .register(None, fixture.registration(0))
            .await
            .unwrap()
            .value;
        let first = producer
            .write_items(
                None,
                handle.stream_id,
                0,
                StreamItemsPayload::PackedU8(vec![1; 64]),
            )
            .await
            .unwrap()
            .value;
        let second = producer
            .write_items(
                None,
                handle.stream_id,
                64,
                StreamItemsPayload::PackedU8(vec![2; 64]),
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
    async fn prefix_alias_lookup_caches_presence_and_absence_per_request() {
        let faults = KeyValueStorageFaults::default();
        let storage = Arc::new(FaultInjectingKeyValueStorage::new(
            Arc::new(InMemoryKeyValueStorage::new()),
            faults.clone(),
        ));
        let fixture = Fixture::with_storage(false, storage.clone()).await;
        let producer = fixture.producer().await;
        let handle = producer
            .register(None, fixture.registration(0))
            .await
            .unwrap()
            .value;
        let offsets = producer
            .write_items(
                None,
                handle.stream_id,
                0,
                StreamItemsPayload::PackedU8(vec![7; 128]),
            )
            .await
            .unwrap()
            .value;
        fixture.persist().await;
        let owner = OwnedAgentId::new(fixture.identity.environment_id, &fixture.identity.agent_id);
        let target = StreamId(uuid::Uuid::from_u128(42));
        for (stream, expected_reads) in [(handle.stream_id, 1), (target, 2)] {
            if stream == target {
                storage
                    .with_entity("test", "seed_alias", "metadata")
                    .set(
                        KeyValueStorageNamespace::AgentDurableStreamSessionIndex {
                            agent_id: owner.agent_id.clone(),
                        },
                        &ProducerMetadataKey::PrefixAlias(target).field().unwrap(),
                        &ProducerMetadataRow::PrefixAlias {
                            previous_stream_id: handle.stream_id,
                            retained_next_sequence: 64,
                        },
                    )
                    .await
                    .unwrap();
            }
            // A second request also proves that cached absence is not kept globally.
            for _ in 0..2 {
                let before = faults.calls("producer_prefix_alias");
                let (_, rows) = fixture
                    .service
                    .lookup_durable_stream_producer_metadata(
                        &owner,
                        AgentMode::Durable,
                        (0..125)
                            .map(|sequence| ProducerMetadataKey::Batch(stream, sequence))
                            .collect(),
                    )
                    .await
                    .unwrap();
                assert_eq!(rows.len(), 125);
                assert!(
                    matches!(rows[0], Some(ProducerMetadataRow::Batch(index)) if index == offsets[0].producer_oplog_index())
                );
                assert!(rows[1..].iter().all(Option::is_none));
                assert_eq!(
                    faults.calls("producer_prefix_alias") - before,
                    expected_reads
                );
            }
        }
    }

    #[test]
    #[timeout("60s")]
    async fn cold_ordinary_segment_loads_batch_locators_in_linear_windows() {
        let fixture = Fixture::new().await;
        let producer = fixture.producer().await;
        let handle = producer
            .register(None, fixture.registration(0))
            .await
            .unwrap()
            .value;
        for sequence in 0..256 {
            producer
                .write_items(
                    None,
                    handle.stream_id,
                    sequence,
                    StreamItemsPayload::Values(vec![vec![sequence as u8; 64]]),
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
            .register(None, fixture.registration(0))
            .await
            .unwrap()
            .value;
        let mut offsets = Vec::new();
        for sequence in 0..140 {
            offsets.push(
                producer
                    .write_items(
                        None,
                        handle.stream_id,
                        sequence,
                        StreamItemsPayload::Values(vec![vec![sequence as u8]]),
                    )
                    .await
                    .unwrap()
                    .value[0],
            );
        }
        let terminal_offset = producer
            .end(None, handle.stream_id, 140, StreamEndResult::Ok)
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
                && event.payload == CommittedProducerStreamEventPayload::Value(vec![sequence as u8])
        }));
        assert!(matches!(
            &page.last().unwrap().payload,
            CommittedProducerStreamEventPayload::End(StreamEndResult::Ok)
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
        let producer = DurableStreamStore::load(
            fixture.oplog.clone(),
            fixture.identity.environment_id,
            fixture.identity.agent_id.clone(),
            fixture.identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let handle = producer
            .register(None, fixture.registration(0))
            .await
            .unwrap()
            .value;
        for _ in 0..1021 {
            fixture.oplog.add(OplogEntry::interrupted()).await;
        }
        assert_eq!(fixture.oplog.current_oplog_index().await.as_u64(), 1023);
        let nested = registration(
            &fixture.identity,
            StreamRegistrationCoordinate::Nested {
                parent_stream_id: handle.stream_id,
                parent_producer_sequence: 0,
                recursive_value_path: vec![
                    golem_common::model::durable_stream::StreamValuePathStep::OptionSome,
                ],
            },
            StreamSourceKind::Nested,
        );
        producer
            .write_items_with_nested(
                None,
                handle.stream_id,
                0,
                StreamItemsPayload::Values(vec![vec![1; 128]]),
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
            .register(None, fixture.registration(0))
            .await
            .unwrap()
            .value;
        let nested = (0..64)
            .map(|ordinal| {
                registration(
                    &fixture.identity,
                    StreamRegistrationCoordinate::Nested {
                        parent_stream_id: handle.stream_id,
                        parent_producer_sequence: 0,
                        recursive_value_path: vec![
                            golem_common::model::durable_stream::StreamValuePathStep::TupleElement(
                                ordinal,
                            ),
                        ],
                    },
                    StreamSourceKind::Nested,
                )
            })
            .collect();
        producer
            .write_items_with_nested(
                None,
                handle.stream_id,
                0,
                StreamItemsPayload::Values(vec![vec![1; 128]]),
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
            .register(None, fixture.registration(0))
            .await
            .unwrap()
            .value;
        producer
            .write_items(
                None,
                handle.stream_id,
                0,
                StreamItemsPayload::PackedU8(vec![7]),
            )
            .await
            .unwrap();
        let terminal_offset = producer
            .end(None, handle.stream_id, 1, StreamEndResult::Ok)
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
                StreamAttachmentFinalizationReason::ConsumerFinalized,
                200,
            )
            .await
            .unwrap();
        fixture.persist().await;
        drop(producer);
        let cold = fixture.producer().await;
        let mut found = HashSet::new();
        for _ in 0..5 {
            let page = cold
                .indexed_attachment_candidates(32)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(page.candidates.len(), 32);
            for candidate in page.candidates {
                let attachment = candidate.attachment;
                let summary = candidate.journal_summary;
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
    async fn persisted_fork_projection_keeps_prefix_locators_and_post_marker_appends() {
        use golem_common::model::durable_stream::{
            AttemptId, StreamForkSessionMapping, StreamForkStreamMapping,
        };
        let fixture = Fixture::new().await;
        let mut source = identity();
        source.agent_id.agent_id = "source".into();
        source.fingerprint = AgentFingerprint(uuid::Uuid::from_u128(90));
        source.invocation.callee = source.agent_id.clone();
        source.invocation.callee_fingerprint = source.fingerprint;
        let registration = registration_record(
            OplogIndex::from_u64(3),
            source.environment_id,
            source.agent_id.clone(),
            source.fingerprint,
            registration(
                &source,
                StreamRegistrationCoordinate::Root {
                    invocation_id: source.invocation.clone(),
                    root_kind: StreamRootKind::MethodResult,
                    recursive_value_path: vec![],
                },
                StreamSourceKind::InvocationOutput,
            ),
        );
        let old = registration.handle.clone();
        let attachment =
            crate::durable_host::durable_stream::tests::attachment_key(&source, old.stream_id);
        let mut cut = StreamForkCutRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            request_hash: vec![0; 32],
            export: None,
            source_environment_id: source.environment_id,
            source: source.agent_id.clone(),
            source_fingerprint: source.fingerprint,
            target_environment_id: fixture.identity.environment_id,
            target: fixture.identity.agent_id.clone(),
            target_fingerprint: fixture.identity.fingerprint,
            cut_index: OplogIndex::from_u64(5),
            revert: None,
            epoch_floor: 1,
            selected_stream_id: Some(old.stream_id),
            retained_through: Some(StreamOffset::new(OplogIndex::from_u64(4), 1)),
            streams: vec![],
            sessions: vec![StreamForkSessionMapping {
                source: source.invocation.clone(),
                continuation: fixture.identity.invocation.clone(),
                continuation_attempt_id: AttemptId::fresh(),
            }],
        };
        let mut next = old.clone();
        next.stream_id =
            crate::services::worker_fork::lineage::continuation_stream_id(&cut, old.stream_id)
                .unwrap();
        next.producer = fixture.identity.agent_id.clone();
        next.expected_producer_fingerprint = fixture.identity.fingerprint;
        next.source_invocation = fixture.identity.invocation.clone();
        cut.streams.push(StreamForkStreamMapping {
            source: old.clone(),
            continuation: next.clone(),
        });
        let items = |position, stream_id, producer_fingerprint, first_sequence, bytes: Vec<u8>| {
            OplogEntry::StreamItems {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                record: OplogPayload::Inline(Box::new(StreamItemsRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    stream_id,
                    producer_fingerprint,
                    first_sequence,
                    offsets: (0..bytes.len())
                        .map(|sub| StreamOffset::new(OplogIndex::from_u64(position), sub as u32))
                        .collect(),
                    nested_stream_ids: vec![],
                    newly_registered_stream_ids: vec![],
                    payload: StreamItemsPayload::PackedU8(bytes),
                })),
            }
        };
        for entry in [
            OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                record: OplogPayload::Inline(Box::new(StreamSessionRecord::Prepared(
                    crate::services::worker_fork::lineage::tests::prepared(&source.invocation),
                ))),
            },
            OplogEntry::StreamRegistered {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                record: OplogPayload::Inline(Box::new(registration)),
            },
            items(
                4,
                old.stream_id,
                source.fingerprint,
                0,
                vec![10, 11, 12, 13, 14],
            ),
            OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                record: OplogPayload::Inline(Box::new(StreamSessionRecord::AttachmentPrepared(
                    golem_common::model::durable_stream::StreamAttachmentPreparedRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        key: attachment.clone(),
                        prepared_at_millis: 100,
                        lease_expires_at_millis: 1000,
                    },
                ))),
            },
            OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                record: OplogPayload::Inline(Box::new(StreamSessionRecord::ForkCut(cut))),
            },
            items(
                7,
                next.stream_id,
                fixture.identity.fingerprint,
                2,
                vec![40, 41],
            ),
        ] {
            fixture.oplog.add(entry).await;
        }
        fixture.persist().await;
        let owner = OwnedAgentId::new(fixture.identity.environment_id, &fixture.identity.agent_id);
        let (_, rows) = fixture
            .service
            .lookup_durable_stream_producer_metadata(
                &owner,
                AgentMode::Durable,
                vec![
                    ProducerMetadataKey::Stream(old.stream_id),
                    ProducerMetadataKey::Stream(next.stream_id),
                    ProducerMetadataKey::Batch(next.stream_id, 0),
                    ProducerMetadataKey::Position(next.stream_id, OplogIndex::from_u64(4)),
                    ProducerMetadataKey::Batch(next.stream_id, 2),
                    ProducerMetadataKey::SessionPage(0),
                    ProducerMetadataKey::Session(source.invocation),
                    ProducerMetadataKey::Attachment(
                        attachment.attachment_id,
                        old.stream_id,
                        attachment.consumer_environment_id,
                        attachment.consumer.clone(),
                    ),
                    ProducerMetadataKey::AttachmentPage(0),
                    ProducerMetadataKey::Global,
                ],
            )
            .await
            .unwrap();
        assert!(rows[0].is_none());
        assert!(
            matches!(&rows[1], Some(ProducerMetadataRow::Stream(state)) if state.next_sequence == 4 && !state.terminal)
        );
        assert!(
            matches!(rows[2], Some(ProducerMetadataRow::Batch(position)) if position == OplogIndex::from_u64(4))
        );
        assert!(matches!(rows[3], Some(ProducerMetadataRow::Position(0, 2))));
        assert!(
            matches!(rows[4], Some(ProducerMetadataRow::Batch(position)) if position == OplogIndex::from_u64(7))
        );
        assert!(
            matches!(&rows[5], Some(ProducerMetadataRow::SessionPage(sessions)) if sessions.iter().cloned().collect::<HashSet<_>>() == HashSet::from([fixture.identity.invocation.clone(), attachment.session_key.clone()]))
        );
        assert!(rows[6].is_none());
        assert!(rows[7].is_none());
        assert!(
            matches!(&rows[8], Some(ProducerMetadataRow::AttachmentPage(page)) if page.is_empty())
        );
        assert!(matches!(
            &rows[9],
            Some(ProducerMetadataRow::Global {
                active_attachments: 0,
                ..
            })
        ));
        let mut fresh = attachment.clone();
        fresh.stream_id = next.stream_id;
        fresh.producer = next.producer.clone();
        fresh.expected_producer_fingerprint = next.expected_producer_fingerprint;
        let producer = fixture.producer().await;
        let prefix = producer.read_segment(&next, None, None).await.unwrap();
        assert_eq!(
            prefix
                .iter()
                .map(|event| event.payload.clone())
                .collect::<Vec<_>>(),
            [10, 11, 40, 41]
                .into_iter()
                .map(CommittedProducerStreamEventPayload::PackedU8)
                .collect::<Vec<_>>()
        );
        assert_eq!(
            prefix.iter().map(|event| event.offset).collect::<Vec<_>>(),
            vec![
                StreamOffset::new(OplogIndex::from_u64(4), 0),
                StreamOffset::new(OplogIndex::from_u64(4), 1),
                StreamOffset::new(OplogIndex::from_u64(7), 0),
                StreamOffset::new(OplogIndex::from_u64(7), 1),
            ]
        );
        assert!(prefix.iter().all(|event| event.stream_id == next.stream_id));
        assert_eq!(prefix[0].packed_u8_batch_end, Some(prefix[1].offset));
        assert_eq!(prefix[2].packed_u8_batch_end, Some(prefix[3].offset));
        producer
            .prepare_attachment(fresh.clone(), 200)
            .await
            .unwrap();
        producer
            .activate_attachment(fresh.clone(), 210)
            .await
            .unwrap();
        fixture.persist().await;
        let (_, rows) = fixture
            .service
            .lookup_durable_stream_producer_metadata(
                &owner,
                AgentMode::Durable,
                vec![
                    ProducerMetadataKey::AttachmentPage(0),
                    ProducerMetadataKey::AttachmentPosition(
                        fresh.attachment_id,
                        fresh.stream_id,
                        fresh.consumer_environment_id,
                        fresh.consumer.clone(),
                    ),
                    ProducerMetadataKey::Global,
                ],
            )
            .await
            .unwrap();
        assert!(
            matches!(&rows[0], Some(ProducerMetadataRow::AttachmentPage(page)) if page == &vec![(fresh.attachment_id, fresh.stream_id, fresh.consumer_environment_id, fresh.consumer.clone())])
        );
        assert!(matches!(
            &rows[1],
            Some(ProducerMetadataRow::AttachmentPosition(Some(0)))
        ));
        assert!(matches!(
            &rows[2],
            Some(ProducerMetadataRow::Global {
                active_attachments: 1,
                ..
            })
        ));
        producer
            .finalize_attachment(
                fresh.clone(),
                StreamAttachmentFinalizationReason::ConsumerFinalized,
                220,
            )
            .await
            .unwrap();
        fixture.persist().await;
        let (_, rows) = fixture
            .service
            .lookup_durable_stream_producer_metadata(
                &owner,
                AgentMode::Durable,
                vec![
                    ProducerMetadataKey::AttachmentPage(0),
                    ProducerMetadataKey::Global,
                ],
            )
            .await
            .unwrap();
        assert!(
            matches!(&rows[0], Some(ProducerMetadataRow::AttachmentPage(page)) if page.is_empty())
        );
        assert!(matches!(
            &rows[1],
            Some(ProducerMetadataRow::Global {
                active_attachments: 0,
                ..
            })
        ));
    }

    #[test]
    #[timeout("60s")]
    async fn cold_fork_terminal_reads_use_continuation_identity() {
        use golem_common::model::durable_stream::{
            AttemptId, StreamForkSessionMapping, StreamForkStreamMapping,
        };
        let fixture = Fixture::new().await;
        fixture
            .oplog
            .add(OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                record: OplogPayload::Inline(Box::new(StreamSessionRecord::Prepared(
                    crate::services::worker_fork::lineage::tests::prepared(
                        &fixture.identity.invocation,
                    ),
                ))),
            })
            .await;
        let producer = fixture.producer().await;
        let original = producer
            .register(None, fixture.registration(0))
            .await
            .unwrap()
            .value;
        let terminal = producer
            .end(None, original.stream_id, 0, StreamEndResult::Ok)
            .await
            .unwrap()
            .value;
        fixture.persist().await;
        drop(producer);
        let mut cut = StreamForkCutRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            request_hash: vec![0; 32],
            export: None,
            source_environment_id: fixture.identity.environment_id,
            source: fixture.identity.agent_id.clone(),
            source_fingerprint: fixture.identity.fingerprint,
            target_environment_id: fixture.identity.environment_id,
            target: fixture.identity.agent_id.clone(),
            target_fingerprint: fixture.identity.fingerprint,
            cut_index: fixture.oplog.current_oplog_index().await,
            revert: None,
            epoch_floor: 1,
            selected_stream_id: None,
            retained_through: None,
            streams: vec![],
            sessions: vec![StreamForkSessionMapping {
                source: fixture.identity.invocation.clone(),
                continuation: fixture.identity.invocation.clone(),
                continuation_attempt_id: AttemptId::fresh(),
            }],
        };
        let mut continuation = original.clone();
        continuation.stream_id =
            crate::services::worker_fork::lineage::continuation_stream_id(&cut, original.stream_id)
                .unwrap();
        cut.streams.push(StreamForkStreamMapping {
            source: original,
            continuation: continuation.clone(),
        });
        fixture
            .oplog
            .add(OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                record: OplogPayload::Inline(Box::new(StreamSessionRecord::ForkCut(cut))),
            })
            .await;
        fixture.persist().await;
        let cold = fixture.producer().await;
        let events = cold.read_segment(&continuation, None, None).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].stream_id, continuation.stream_id);
        assert_eq!(events[0].offset, terminal);
        assert_eq!(
            events[0].payload,
            CommittedProducerStreamEventPayload::End(StreamEndResult::Ok)
        );
    }

    #[test]
    #[timeout("60s")]
    async fn raw_and_indexed_controls_discard_nested_descendants_at_a_fork() {
        use crate::durable_host::durable_session::StreamSession;
        use crate::durable_host::durable_stream::tests::TestOplog;
        use crate::services::worker_fork::lineage::tests::{fork, prepared};
        use golem_common::model::durable_stream::{
            StreamConsumerItemValueRecord, StreamSessionMappingUpdateRecord,
        };

        let fixture =
            Fixture::with_buffer(false, Arc::new(InMemoryKeyValueStorage::new()), 100).await;
        let target = &fixture.identity;
        let owner = OwnedAgentId::new(target.environment_id, &target.agent_id);
        let mut source = identity();
        source.agent_id.agent_id = "source".into();
        source.invocation.callee = source.agent_id.clone();
        let oplog = Arc::new(TestOplog::default());
        oplog
            .add(fixture.oplog.read(OplogIndex::INITIAL).await)
            .await;
        let producer = DurableStreamStore::load(
            oplog.clone(),
            source.environment_id,
            source.agent_id.clone(),
            source.fingerprint,
            None,
        )
        .await
        .unwrap();
        producer
            .append_session_record(
                None,
                StreamSessionRecord::Prepared(prepared(&source.invocation)),
            )
            .await
            .unwrap();
        let root = producer
            .register(
                None,
                registration(
                    &source,
                    StreamRegistrationCoordinate::Root {
                        invocation_id: source.invocation.clone(),
                        root_kind: StreamRootKind::MethodResult,
                        recursive_value_path: vec![],
                    },
                    StreamSourceKind::InvocationOutput,
                ),
            )
            .await
            .unwrap()
            .value;
        let retained = producer
            .write_items(
                None,
                root.stream_id,
                0,
                StreamItemsPayload::PackedU8(vec![7, 9]),
            )
            .await
            .unwrap()
            .value[0];
        let mut handles = vec![root.clone()];
        for (parent, sequence) in [(0, 2), (1, 0)] {
            let parent = handles[parent].stream_id;
            producer
                .write_items_with_nested(
                    None,
                    parent,
                    sequence,
                    StreamItemsPayload::Values(vec![vec![1]]),
                    vec![registration(
                        &source,
                        StreamRegistrationCoordinate::Nested {
                            parent_stream_id: parent,
                            parent_producer_sequence: sequence,
                            recursive_value_path: vec![],
                        },
                        StreamSourceKind::Nested,
                    )],
                )
                .await
                .unwrap();
            handles.push(producer.nested_handles(parent, sequence).await.unwrap()[0].clone());
        }
        for (number, handle) in handles.iter().enumerate() {
            producer
                .append_session_record(
                    None,
                    StreamSessionRecord::Mapping(StreamSessionMappingUpdateRecord {
                        format_version: 1,
                        session_key: source.invocation.clone(),
                        mapping: StreamSessionMappingRecord {
                            transport_stream_id: number as u64,
                            handle: handle.clone(),
                            role: SessionStreamRole::Output,
                        },
                    }),
                )
                .await
                .unwrap();
            producer
                .append_session_record(
                    None,
                    StreamSessionRecord::ConsumerItemValue(StreamConsumerItemValueRecord {
                        format_version: 1,
                        session_key: source.invocation.clone(),
                        stream_id: handle.stream_id,
                        source_offset: retained,
                        consumer_read_ordinal: 0,
                        value: vec![7],
                        packed_u8: true,
                        recursive_handles: vec![],
                        recursive_mappings: vec![],
                    }),
                )
                .await
                .unwrap();
        }
        let mut cut = fork(
            &root,
            &owner,
            target.fingerprint,
            oplog.current_oplog_index().await.as_u64(),
        );
        cut.retained_through = Some(retained);
        for handle in &handles[1..] {
            let mapped = fork(handle, &owner, target.fingerprint, cut.cut_index.as_u64());
            cut.streams.extend(mapped.streams);
        }
        let target_key = cut.sessions[0].continuation.clone();
        let retained_id = cut.streams[0].continuation.stream_id;
        oplog
            .add(OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                record: OplogPayload::Inline(Box::new(StreamSessionRecord::ForkCut(cut))),
            })
            .await;
        for (_, entry) in oplog
            .read_exact(
                OplogIndex::INITIAL.next(),
                oplog.current_oplog_index().await.as_u64() - 1,
            )
            .await
        {
            fixture.oplog.add(entry).await;
        }
        fixture.persist().await;
        let raw_producer = DurableStreamStore::load(
            oplog.clone(),
            owner.environment_id,
            owner.agent_id.clone(),
            target.fingerprint,
            None,
        )
        .await
        .unwrap();
        let raw = StreamSession::new(raw_producer, oplog, target_key.clone(), []);
        let raw = raw.current_control_metadata().await.unwrap();
        let mut indexed = fixture
            .service
            .lookup_durable_stream_control_metadata(&owner, AgentMode::Durable, &target_key)
            .await
            .unwrap();
        indexed.assign_recovery_slot(None);
        assert_eq!(*raw, indexed);
        assert_eq!(
            raw.acceptance_mappings()
                .unwrap()
                .iter()
                .map(|mapping| mapping.handle.stream_id)
                .collect::<Vec<_>>(),
            vec![retained_id]
        );
        assert_eq!(
            raw.consumer_record_counts(),
            &HashMap::from([(retained_id, 1)])
        );
    }

    #[test]
    #[timeout("60s")]
    async fn persisted_fork_chain_preserves_mapping_free_consumer_heads() {
        use golem_common::model::durable_stream::{
            AttemptId, StreamConsumerItemValueRecord, StreamConsumerTerminal,
            StreamConsumerTerminalRecord, StreamForkSessionMapping,
        };
        for terminal_record in [false, true] {
            let fixture =
                Fixture::with_buffer(false, Arc::new(InMemoryKeyValueStorage::new()), 100).await;
            let target = fixture.identity.invocation.clone();
            let mut source = target.clone();
            source.callee.agent_id = "source".into();
            source.callee_fingerprint = AgentFingerprint(uuid::Uuid::from_u128(91));
            let mut middle = target.clone();
            middle.callee.agent_id = "middle".into();
            middle.callee_fingerprint = AgentFingerprint(uuid::Uuid::from_u128(92));
            let stream = StreamId(uuid::Uuid::from_u128(93));
            let terminal = StreamOffset::new(OplogIndex::from_u64(91), 0);
            let cut = |from: &StreamSessionKey, to: &StreamSessionKey, index| {
                StreamSessionRecord::ForkCut(StreamForkCutRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    request_hash: vec![0; 32],
                    export: None,
                    source_environment_id: from.callee_environment_id,
                    source: from.callee.clone(),
                    source_fingerprint: from.callee_fingerprint,
                    target_environment_id: to.callee_environment_id,
                    target: to.callee.clone(),
                    target_fingerprint: to.callee_fingerprint,
                    cut_index: OplogIndex::from_u64(index),
                    revert: None,
                    epoch_floor: 1,
                    selected_stream_id: None,
                    retained_through: None,
                    streams: vec![],
                    sessions: vec![StreamForkSessionMapping {
                        source: from.clone(),
                        continuation: to.clone(),
                        continuation_attempt_id: AttemptId::fresh(),
                    }],
                })
            };
            for record in [
                StreamSessionRecord::Prepared(
                    crate::services::worker_fork::lineage::tests::prepared(&source),
                ),
                StreamSessionRecord::ConsumerItemValue(StreamConsumerItemValueRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: source.clone(),
                    stream_id: stream,
                    source_offset: StreamOffset::new(OplogIndex::from_u64(90), 3),
                    consumer_read_ordinal: 0,
                    value: vec![3, 5],
                    packed_u8: true,
                    recursive_handles: vec![],
                    recursive_mappings: vec![],
                }),
                if terminal_record {
                    StreamSessionRecord::ConsumerTerminal(StreamConsumerTerminalRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: source.clone(),
                        stream_id: stream,
                        source_offset: terminal,
                        consumer_read_ordinal: 2,
                        terminal: StreamConsumerTerminal::End(StreamEndResult::Ok),
                    })
                } else {
                    StreamSessionRecord::ConsumerItemValue(StreamConsumerItemValueRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: source.clone(),
                        stream_id: stream,
                        source_offset: terminal,
                        consumer_read_ordinal: 2,
                        value: vec![7],
                        packed_u8: true,
                        recursive_handles: vec![],
                        recursive_mappings: vec![],
                    })
                },
                cut(&source, &middle, 4),
                cut(&middle, &target, 5),
            ] {
                let prepared = matches!(&record, StreamSessionRecord::Prepared(_));
                fixture
                    .oplog
                    .add(OplogEntry::StreamSession {
                        timestamp: Timestamp::now_utc(),
                        entity_parent_start_index: None,
                        record: OplogPayload::Inline(Box::new(record)),
                    })
                    .await;
                if prepared {
                    assert!(
                        fixture
                            .oplog
                            .raw_durable_stream_session_status(&source)
                            .await
                            .status
                            .unwrap()
                            .is_some()
                    );
                    assert!(
                        fixture
                            .oplog
                            .raw_durable_stream_session_status(&target)
                            .await
                            .status
                            .unwrap()
                            .is_none()
                    );
                }
            }
            let buffered = fixture
                .oplog
                .raw_durable_stream_session_status(&target)
                .await
                .status
                .unwrap()
                .unwrap();
            assert_eq!(buffered.session_key.as_ref(), Some(&target));
            assert_eq!(buffered.attachment_epoch, Some(1));
            assert_eq!(buffered.attachment_attached, Some(false));
            assert!(
                fixture
                    .oplog
                    .raw_durable_stream_session_status(&source)
                    .await
                    .status
                    .unwrap()
                    .is_none()
            );
            fixture.persist().await;
            use crate::services::HasOplogService;
            let owner =
                OwnedAgentId::new(fixture.identity.environment_id, &fixture.identity.agent_id);
            let persisted = fixture
                .service
                .oplog_service()
                .stream_session_index()
                .unwrap()
                .lookup_latest(&owner, AgentMode::Durable, &target.idempotency_key)
                .await
                .unwrap();
            assert_eq!(persisted, Some(buffered.clone()));
            assert_eq!(
                fixture
                    .oplog
                    .raw_durable_stream_session_status(&target)
                    .await
                    .status
                    .unwrap(),
                Some(buffered)
            );
            let owner =
                OwnedAgentId::new(fixture.identity.environment_id, &fixture.identity.agent_id);
            let (_, rows) = fixture
                .service
                .lookup_durable_stream_producer_metadata(
                    &owner,
                    AgentMode::Durable,
                    vec![
                        ProducerMetadataKey::ConsumerHead(source.clone(), stream),
                        ProducerMetadataKey::ConsumerHead(middle.clone(), stream),
                        ProducerMetadataKey::ConsumerHead(target.clone(), stream),
                    ],
                )
                .await
                .unwrap();
            assert!(rows[0].is_none());
            assert!(rows[1].is_none());
            assert!(
                matches!(&rows[2], Some(ProducerMetadataRow::ConsumerHead(head)) if head.terminal == terminal_record && head.next_read_ordinal == 3 && head.last_source_offset == Some(terminal))
            );
            let control = fixture
                .service
                .lookup_durable_stream_control_metadata(&owner, AgentMode::Durable, &target)
                .await
                .unwrap();
            assert_eq!(control.prepared_position(), Some(OplogIndex::from_u64(2)));
            assert_eq!(
                control.consumer_record_counts(),
                &HashMap::from([(stream, 2)])
            );
            let raw_producer = DurableStreamStore::load(
                fixture.oplog.clone(),
                owner.environment_id,
                owner.agent_id.clone(),
                fixture.identity.fingerprint,
                None,
            )
            .await
            .unwrap();
            let raw = crate::durable_host::durable_session::StreamSession::new(
                raw_producer.clone(),
                fixture.oplog.clone(),
                target.clone(),
                [],
            );
            let raw_control = raw.current_control_metadata().await.unwrap();
            let mut expected = control.clone();
            expected.assign_recovery_slot(None);
            assert_eq!(*raw_control, expected);
            assert_eq!(
                fixture
                    .service
                    .read_durable_stream_consumer_page(&owner, &target, stream, 0)
                    .await
                    .unwrap(),
                vec![OplogIndex::from_u64(3), OplogIndex::from_u64(4)]
            );
            for retired in [&source, &middle] {
                let raw = crate::durable_host::durable_session::StreamSession::new(
                    raw_producer.clone(),
                    fixture.oplog.clone(),
                    retired.clone(),
                    [],
                );
                let raw_control = raw.current_control_metadata().await.unwrap();
                assert!(raw_control.prepared_position().is_none());
                assert!(raw_control.consumer_record_counts().is_empty());
                let control = fixture
                    .service
                    .lookup_durable_stream_control_metadata(&owner, AgentMode::Durable, retired)
                    .await
                    .unwrap();
                assert!(control.prepared_position().is_none());
                assert!(control.consumer_record_counts().is_empty());
            }
            let recovery = fixture
                .service
                .lookup_durable_stream_recovery_metadata(&owner, AgentMode::Durable)
                .await
                .unwrap();
            assert_eq!(
                recovery
                    .sessions
                    .iter()
                    .map(|(key, _)| key.clone())
                    .collect::<Vec<_>>(),
                vec![target.clone()]
            );
            if !terminal_record {
                fixture
                    .oplog
                    .add(OplogEntry::StreamSession {
                        timestamp: Timestamp::now_utc(),
                        entity_parent_start_index: None,
                        record: OplogPayload::Inline(Box::new(
                            StreamSessionRecord::ConsumerItemValue(StreamConsumerItemValueRecord {
                                format_version: DURABLE_STREAM_FORMAT_VERSION,
                                session_key: target.clone(),
                                stream_id: stream,
                                source_offset: StreamOffset::new(OplogIndex::from_u64(92), 4),
                                consumer_read_ordinal: 3,
                                value: vec![11, 13],
                                packed_u8: true,
                                recursive_handles: vec![],
                                recursive_mappings: vec![],
                            }),
                        )),
                    })
                    .await;
                fixture.persist().await;
                assert_eq!(
                    fixture
                        .service
                        .read_durable_stream_consumer_page(&owner, &target, stream, 0)
                        .await
                        .unwrap(),
                    vec![
                        OplogIndex::from_u64(3),
                        OplogIndex::from_u64(4),
                        OplogIndex::from_u64(7)
                    ]
                );
                let control = fixture
                    .service
                    .lookup_durable_stream_control_metadata(&owner, AgentMode::Durable, &target)
                    .await
                    .unwrap();
                assert_eq!(
                    control.consumer_record_counts(),
                    &HashMap::from([(stream, 3)])
                );
            }
        }
    }

    #[test]
    #[timeout("60s")]
    async fn persisted_session_catalogue_includes_mapping_free_foreign_sessions_once() {
        let fixture = Fixture::new().await;
        let mut expected = HashSet::new();
        for range in [0..127, 127..130, 0..130] {
            let producer = fixture.producer().await;
            for ordinal in range {
                let mut session = fixture.identity.invocation.clone();
                session.idempotency_key =
                    golem_common::model::IdempotencyKey::new(format!("outbound-{ordinal}"));
                session.callee.agent_id = "foreign()".into();
                let repeated = !expected.insert(session.clone());
                producer
                    .append_session_record(None, StreamSessionRecord::ConsumerItemValue(
                        golem_common::base_model::durable_stream::StreamConsumerItemValueRecord {
                            format_version: DURABLE_STREAM_FORMAT_VERSION,
                            session_key: session,
                            stream_id: StreamId(uuid::Uuid::from_u128(91)),
                            source_offset: StreamOffset::new(
                                OplogIndex::from_u64(if repeated { 2 } else { 1 }),
                                0,
                            ),
                            consumer_read_ordinal: u64::from(repeated),
                            value: vec![7],
                            packed_u8: false,
                            recursive_handles: Vec::new(),
                            recursive_mappings: Vec::new(),
                        },
                    ))
                    .await
                    .unwrap();
            }
            fixture.persist().await;
            drop(producer);
            let owner =
                OwnedAgentId::new(fixture.identity.environment_id, &fixture.identity.agent_id);
            let (_, rows) = fixture
                .service
                .lookup_durable_stream_producer_metadata(
                    &owner,
                    AgentMode::Durable,
                    vec![
                        ProducerMetadataKey::Global,
                        ProducerMetadataKey::SessionPage(0),
                        ProducerMetadataKey::SessionPage(1),
                    ],
                )
                .await
                .unwrap();
            assert!(
                matches!(rows[0], Some(ProducerMetadataRow::Global { session_count, .. }) if session_count == expected.len() as u64)
            );
            let mut actual = Vec::new();
            for (page, row) in rows.into_iter().skip(1).enumerate() {
                if let Some(ProducerMetadataRow::SessionPage(sessions)) = row {
                    assert_eq!(
                        sessions.len(),
                        (expected.len().saturating_sub(page * 128)).min(128)
                    );
                    actual.extend(sessions);
                }
            }
            assert_eq!(actual.len(), expected.len());
            assert_eq!(actual.into_iter().collect::<HashSet<_>>(), expected);
        }
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
                .append_session_record(
                    None,
                    StreamSessionRecord::ConsumerItemValue(
                        golem_common::base_model::durable_stream::StreamConsumerItemValueRecord {
                            format_version: DURABLE_STREAM_FORMAT_VERSION,
                            session_key: session_key.clone(),
                            stream_id,
                            source_offset: StreamOffset::new(OplogIndex::from_u64(ordinal + 1), 0),
                            consumer_read_ordinal: ordinal,
                            value: vec![ordinal as u8],
                            packed_u8: false,
                            recursive_handles: Vec::new(),
                            recursive_mappings: Vec::new(),
                        },
                    ),
                )
                .await
                .unwrap();
        }
        let terminal_offset = StreamOffset::new(OplogIndex::from_u64(513), 0);
        consumer
            .append_session_record(
                None,
                StreamSessionRecord::ConsumerTerminal(
                    golem_common::model::durable_stream::StreamConsumerTerminalRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: session_key.clone(),
                        stream_id,
                        source_offset: terminal_offset,
                        consumer_read_ordinal: 512,
                        terminal: golem_common::model::durable_stream::StreamConsumerTerminal::End(
                            StreamEndResult::Ok,
                        ),
                    },
                ),
            )
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
            .upload_payload(&StreamSessionRecord::ConsumerItemValue(
                golem_common::base_model::durable_stream::StreamConsumerItemValueRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: session.clone(),
                    stream_id: stream,
                    source_offset: StreamOffset::new(OplogIndex::from_u64(17), 0),
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
        fixture
            .oplog
            .add(OplogEntry::stream_session(None, record))
            .await;
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
        assert_eq!(metadata.covered_through(), horizon);
        assert_eq!(metadata.consumer_record_count(stream), 1);
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
            .register(None, fixture.registration(0))
            .await
            .unwrap()
            .value;
        producer
            .end(
                None,
                handle.stream_id,
                0,
                StreamEndResult::ErrorContext(vec![7; 256]),
            )
            .await
            .unwrap();
        fixture.persist().await;
        drop(producer);
        let cold = fixture.producer().await;
        let (started, release) = fixture.blobs.pause_next_read();
        let mut suspended = Box::pin(cold.end(
            None,
            handle.stream_id,
            0,
            StreamEndResult::ErrorContext(vec![7; 256]),
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
    #[timeout("30s")]
    async fn retirement_joins_unary_hydration_before_and_after_parent_cancellation() {
        for cancel_parent in [false, true] {
            let fixture = Fixture::new().await;
            let producer = fixture.producer().await;
            let handle = producer
                .register(None, fixture.registration(0))
                .await
                .unwrap()
                .value;
            producer
                .end(
                    None,
                    handle.stream_id,
                    0,
                    StreamEndResult::ErrorContext(vec![7; 256]),
                )
                .await
                .unwrap();
            fixture.persist().await;
            let cold = fixture.producer().await;
            let late = fixture.producer().await;
            let (started, release) = fixture.blobs.pause_next_read();
            let mut unary = Some(Box::pin(cold.index_for_terminal([], handle.stream_id)));
            assert!(futures::poll!(unary.as_mut().unwrap().as_mut()).is_pending());
            started.await.unwrap();
            let mut stop = Box::pin(fixture.oplog.stop_and_wait());
            assert!(
                futures::poll!(stop.as_mut()).is_pending(),
                "retirement must observe the task before its parent is cancelled"
            );
            assert!(late.index_for_terminal([], handle.stream_id).await.is_err());
            assert!(late.indexed_attachment_candidates(32).await.is_err());
            if cancel_parent {
                drop(unary.take());
            }
            assert!(futures::poll!(stop.as_mut()).is_pending());
            release.send(()).unwrap();
            stop.await.unwrap();
            if let Some(unary) = unary {
                let index = unary.await.unwrap();
                assert!(index.streams[&handle.stream_id].terminal_event.is_some());
            }
            assert!(
                cold.index.try_lock().is_ok(),
                "hydration must release its actual lock"
            );
            assert_eq!(fixture.blobs.reads(), 1);
        }
    }

    #[test]
    #[timeout("30s")]
    async fn cached_queries_need_no_task_admission_after_root_stop() {
        let fixture = Fixture::new().await;
        let producer = fixture.producer().await;
        let handle = producer
            .register(None, fixture.registration(0))
            .await
            .unwrap()
            .value;
        let expected = producer.input_high_water(handle.stream_id).await.unwrap();
        let reads = fixture.indexed.reads();
        producer.tasks().stop_roots_and_wait().await;
        assert!(
            producer.indexed_attachment_candidates(1).await.is_ok(),
            "owner cleanup still needs metadata queries after roots stop"
        );
        producer.tasks().stop_and_wait().await.unwrap();
        let reads_after_cleanup = fixture.indexed.reads();
        assert!(reads_after_cleanup >= reads);
        for _ in 0..32 {
            assert_eq!(
                producer.input_high_water(handle.stream_id).await.unwrap(),
                expected
            );
        }
        assert_eq!(fixture.indexed.reads(), reads_after_cleanup);
        assert!(producer.indexed_attachment_candidates(1).await.is_err());
    }

    #[test]
    #[ignore]
    async fn metadata_task_admission_workloads() {
        let fixture = Fixture::new().await;
        let producer = fixture.producer().await;
        let worker = Arc::new(());
        producer.set_worker_tasks(
            fixture
                .oplog
                .task_owner()
                .unwrap()
                .for_worker(worker.clone()),
        );
        let handle = producer
            .register(None, fixture.registration(0))
            .await
            .unwrap()
            .value;
        producer
            .write_items(
                None,
                handle.stream_id,
                0,
                StreamItemsPayload::PackedU8(vec![17, 3, 29]),
            )
            .await
            .unwrap();
        producer
            .end(
                None,
                handle.stream_id,
                3,
                StreamEndResult::ErrorContext(vec![7; 256]),
            )
            .await
            .unwrap();
        let attachment = crate::durable_host::durable_stream::tests::attachment_key(
            &fixture.identity,
            handle.stream_id,
        );
        producer.prepare_attachment(attachment, 100).await.unwrap();
        let attempt_id = golem_common::model::durable_stream::AttemptId::fresh();
        producer
            .append_session_record(
                None,
                StreamSessionRecord::CallerAttempt(
                    golem_common::model::durable_stream::StreamCallerAttemptRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: fixture.identity.invocation.clone(),
                        attempt_id,
                    },
                ),
            )
            .await
            .unwrap();
        fixture.persist().await;

        for workload in [
            "cached-query",
            "cold-hydration",
            "attachment-query",
            "session-resume",
        ] {
            for sample in 0..=10 {
                for tracked in if sample % 2 == 0 {
                    [false, true]
                } else {
                    [true, false]
                } {
                    let iterations = if sample == 0 {
                        100
                    } else if workload == "cached-query" {
                        100_000
                    } else {
                        1_000
                    };
                    let reads = fixture.indexed.reads();
                    let blobs = fixture.blobs.reads();
                    let start = std::time::Instant::now();
                    for _ in 0..iterations {
                        match workload {
                            "cached-query" => {
                                assert!(
                                    producer
                                        .input_high_water(handle.stream_id)
                                        .await
                                        .unwrap()
                                        .is_some()
                                );
                            }
                            "cold-hydration" => {
                                *producer.index.lock().await = ProducerStreamIndex::default();
                                let query = producer.clone().hydrate_query(
                                    vec![ProducerMetadataKey::Stream(handle.stream_id)],
                                    Some(handle.stream_id),
                                    None,
                                );
                                let task = if tracked {
                                    producer.tasks().spawn_metadata(query).unwrap()
                                } else {
                                    tokio::spawn(query)
                                };
                                task.await.unwrap().unwrap();
                                assert!(
                                    producer.index.lock().await.streams[&handle.stream_id]
                                        .terminal_event
                                        .is_some()
                                );
                            }
                            "attachment-query" => {
                                let producer_for_query = producer.clone();
                                let query = async move {
                                    producer_for_query
                                        .indexed_attachment_candidates_inner(32)
                                        .await
                                };
                                let task = if tracked {
                                    producer.tasks().spawn_metadata(query).unwrap()
                                } else {
                                    tokio::spawn(query)
                                };
                                let IndexedAttachmentCandidateBatch { candidates, .. } =
                                    task.await.unwrap().unwrap().unwrap();
                                assert_eq!(candidates.len(), 1);
                            }
                            "session-resume" => {
                                let streams =
                                    crate::durable_host::durable_session::StreamSession::new(
                                        producer.clone(),
                                        fixture.oplog.clone(),
                                        fixture.identity.invocation.clone(),
                                        [],
                                    );
                                let scope = crate::worker::tasks::TaskScope::default();
                                let resumed = if tracked {
                                    scope.bind(producer.tasks()).unwrap();
                                    scope.run(streams.caller_attempt_id()).await.unwrap()
                                } else {
                                    streams.caller_attempt_id().await
                                };
                                assert_eq!(resumed.unwrap(), attempt_id);
                            }
                            _ => unreachable!(),
                        }
                    }
                    let elapsed = start.elapsed();
                    println!(
                        "workload={workload}, tracked={tracked}, sample={sample}, iterations={iterations}, ns/op={}, indexed_reads={}, blob_reads={}",
                        elapsed.as_nanos() / iterations,
                        fixture.indexed.reads() - reads,
                        fixture.blobs.reads() - blobs
                    );
                }
            }
        }
    }

    #[test]
    async fn suspended_contended_query_does_not_reserve_producer_index() {
        let fixture = Fixture::new().await;
        let producer = fixture.producer().await;
        let handle = producer
            .register(None, fixture.registration(0))
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
            .register(None, fixture.registration(0))
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
