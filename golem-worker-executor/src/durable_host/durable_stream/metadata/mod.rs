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
    Session(StreamSessionKey),
    SessionPage(u64),
    Batch(StreamId, u64),
    Attachment(AttachmentId, StreamId, EnvironmentId, AgentId),
    ActiveAttachmentCount(StreamSessionKey, StreamId),
    ConsumerHead(StreamSessionKey, LocalStreamReaderId),
    Cascade(Box<StreamAttachmentKey>),
    AttachmentPage(u64),
    AttachmentPosition(AttachmentId, StreamId, EnvironmentId, AgentId),
    Position(StreamId, OplogIndex),
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

    pub(super) fn session_record(
        record: &StreamSessionRecord,
        owner_environment_id: EnvironmentId,
        owner: &AgentId,
        owner_fingerprint: AgentFingerprint,
    ) -> Vec<Self> {
        let mut keys = Vec::new();
        let bindings: &[StreamBindingRecord] = match record {
            StreamSessionRecord::Mapping(record) => std::slice::from_ref(&record.mapping),
            StreamSessionRecord::Prepared(record) => &record.stream_mappings,
            StreamSessionRecord::InvocationResult(record) => &record.stream_mappings,
            _ => &[],
        };
        keys.extend(bindings.iter().filter_map(|binding| {
            match binding.source {
                StreamRecordReference::Local(id) => {
                    qualify_local_stream(id, owner_environment_id, owner, owner_fingerprint)
                        .ok()
                        .map(Self::Stream)
                }
                StreamRecordReference::Foreign(_) => None,
            }
        }));
        if let Some(key) = crate::worker::stream_session_record_key(
            record,
            owner_environment_id,
            owner,
            owner_fingerprint,
        ) {
            keys.push(Self::Session(key));
        }
        let attachment = match record {
            StreamSessionRecord::AttachmentPrepared(record) => Some(&record.key),
            StreamSessionRecord::AttachmentActivated(record) => Some(&record.key),
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
            StreamSessionRecord::ConsumerItemValue(record) => Some((
                record
                    .session_key
                    .qualify(owner_environment_id, owner, owner_fingerprint),
                record.reader_id,
            )),
            StreamSessionRecord::ConsumerTerminal(record) => Some((
                record
                    .session_key
                    .qualify(owner_environment_id, owner, owner_fingerprint),
                record.reader_id,
            )),
            StreamSessionRecord::SourceUnavailable(record) => Some((
                record
                    .session_key
                    .qualify(owner_environment_id, owner, owner_fingerprint),
                record.reader_id,
            )),
            _ => None,
        };
        if let Some((session, reader)) = head {
            keys.push(Self::ConsumerHead(session, reader));
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
    registration: RegisteredStream,
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
    mappings: HashSet<(StreamRecordReference, SessionStreamRole)>,
    open_streams: HashSet<StreamId>,
    consumer_streams: HashSet<LocalStreamReaderId>,
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
    Session(ProducerSessionMetadata),
    SessionPage(Vec<StreamSessionKey>),
    Batch(OplogIndex),
    Attachment(Box<IndexedStreamAttachment>),
    ActiveAttachmentCount(u64),
    ConsumerHead(IndexedConsumerJournal),
    Cascade(StreamCascadeDependentResult),
    AttachmentPage(Vec<(AttachmentId, StreamId, EnvironmentId, AgentId)>),
    AttachmentPosition(Option<u64>),
    Position(u64, u64),
    ExternalProducer(IndexedExternalProducer),
    ExternalProducerOffset(StreamOffset),
}

impl ProducerStreamIndex {
    pub(super) fn metadata_row(&self, key: &ProducerMetadataKey) -> Option<ProducerMetadataRow> {
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

struct Projection<'a> {
    index: ProducerStreamIndex,
    loaded: HashSet<ProducerMetadataKey>,
    new_sessions: HashSet<StreamSessionKey>,
    replacements: HashMap<ProducerMetadataKey, ProducerMetadataRow>,
    storage: &'a (dyn KeyValueStorage + Send + Sync),
    namespace: KeyValueStorageNamespace,
}

pub(crate) struct ForkControlProjection {
    pub(crate) sessions: HashSet<StreamSessionKey>,
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
        owner: &OwnedAgentId,
        fingerprint: AgentFingerprint,
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
        let local_ids = self
            .index
            .session_stream_mappings
            .values()
            .flatten()
            .filter_map(|(reference, _)| match reference {
                StreamRecordReference::Local(id) => Some(*id),
                StreamRecordReference::Foreign(_) => None,
            })
            .collect::<HashSet<_>>();
        for local_id in local_ids {
            let stream =
                qualify_local_stream(local_id, owner.environment_id, &owner.agent_id, fingerprint)
                    .map_err(|error| error.to_string())?;
            self.stream(stream).await?;
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
        let retained = match (cut.selected_stream_id, cut.retained_through) {
            (None, None) => None,
            (None, Some(_)) => return Err("fork retained offset has no selected stream".into()),
            (Some(_), None) => Some(0),
            (Some(stream), Some(offset)) => {
                let stream = *self
                    .index
                    .local_stream_ids
                    .get(&stream)
                    .ok_or("fork selected stream is not registered")?;
                let key = ProducerMetadataKey::Position(stream, offset.producer_oplog_index());
                let row = self
                    .storage
                    .with_entity("worker", "producer_projection_read", "position")
                    .get(self.namespace.clone(), &key.field()?)
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
                    | ProducerMetadataKey::Session(_)
                    | ProducerMetadataKey::ConsumerHead(_, _)
            ) && self.index.metadata_row(key).is_none()
            {
                self.replacements
                    .insert(key.clone(), ProducerMetadataRow::Retired);
            }
        }
        for stream in self.index.streams.keys() {
            self.loaded.insert(ProducerMetadataKey::Stream(*stream));
        }
        for coordinate in self.index.coordinates.keys() {
            self.loaded
                .insert(ProducerMetadataKey::Coordinate(coordinate.clone()));
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
            sessions: self
                .index
                .session_entity_parent_start_indices
                .keys()
                .cloned()
                .collect(),
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

    async fn registration(
        &mut self,
        oplog_index: OplogIndex,
        author: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        record: &StreamRegisteredRecord,
    ) -> Result<(), String> {
        let registered = RegisteredStream::resolve(
            record.clone(),
            oplog_index,
            author.environment_id,
            &author.agent_id,
            fingerprint,
        )
        .map_err(|error| error.to_string())?;
        if let StreamRegistrationCoordinate::Nested {
            parent_stream_id, ..
        } = &registered.coordinate
        {
            self.stream(*parent_stream_id).await?;
        }
        self.load(ProducerMetadataKey::Coordinate(
            registered.coordinate.clone(),
        ))
        .await?;
        if let Some(existing) = self.index.coordinates.get(&registered.coordinate).copied() {
            self.stream(existing).await?;
        }
        let stream_id = StreamId::derive(
            author.environment_id,
            &author.agent_id,
            fingerprint,
            oplog_index,
        )
        .map_err(|error| error.to_string())?;
        self.stream(stream_id).await?;
        if let Some(session) = self
            .index
            .registration_session_key(&registered.coordinate, &registered.session_mapping)
        {
            self.load(ProducerMetadataKey::Session(session)).await?;
        }
        Ok(())
    }

    async fn session(
        &mut self,
        record: &StreamSessionRecord,
        author: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> Result<(), String> {
        for key in ProducerMetadataKey::session_record(
            record,
            author.environment_id,
            &author.agent_id,
            fingerprint,
        ) {
            if let ProducerMetadataKey::Stream(stream) = key {
                self.stream(stream).await?;
            }
        }
        if let Some(session) = crate::worker::stream_session_record_key(
            record,
            author.environment_id,
            &author.agent_id,
            fingerprint,
        ) {
            self.load(ProducerMetadataKey::Session(session)).await?;
        }
        let attachment = match record {
            StreamSessionRecord::AttachmentPrepared(record) => Some(&record.key),
            StreamSessionRecord::AttachmentActivated(record) => Some(&record.key),
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
            StreamSessionRecord::ConsumerItemValue(record) => Some((
                record
                    .session_key
                    .qualify(author.environment_id, &author.agent_id, fingerprint),
                record.reader_id,
            )),
            StreamSessionRecord::ConsumerTerminal(record) => Some((
                record
                    .session_key
                    .qualify(author.environment_id, &author.agent_id, fingerprint),
                record.reader_id,
            )),
            StreamSessionRecord::SourceUnavailable(record) => Some((
                record
                    .session_key
                    .qualify(author.environment_id, &author.agent_id, fingerprint),
                record.reader_id,
            )),
            _ => None,
        };
        if let Some((session, reader)) = head {
            self.load(ProducerMetadataKey::ConsumerHead(session, reader))
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
                projection
                    .registration(*index, owner, fingerprint, &record)
                    .await?;
                if matches!(
                    record.coordinate,
                    StreamRegistrationRecordCoordinate::Nested { .. }
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
                            owner.environment_id,
                            &owner.agent_id,
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
                let mut record = oplog.download_payload(owner, mode, record.clone()).await?;
                lineage.project_item_batch(*index, &mut record)?;
                let stream_id = qualify_local_stream(
                    record.stream_id,
                    owner.environment_id,
                    &owner.agent_id,
                    fingerprint,
                )
                .map_err(|error| error.to_string())?;
                projection.stream(stream_id).await?;
                for reference in &record.nested_stream_ids {
                    let stream = materialize_stream_reference(
                        reference,
                        owner.environment_id,
                        &owner.agent_id,
                        fingerprint,
                    )
                    .map_err(|error| error.to_string())?;
                    projection.load(ProducerMetadataKey::Stream(stream)).await?;
                }
                projection
                    .loaded
                    .insert(ProducerMetadataKey::Batch(stream_id, record.first_sequence));
                projection
                    .loaded
                    .insert(ProducerMetadataKey::Position(stream_id, *index));
                projection
                    .index
                    .apply_item_batch(
                        *index,
                        *entity_parent_start_index,
                        std::mem::take(&mut pending),
                        record,
                        owner.environment_id,
                        &owner.agent_id,
                        fingerprint,
                        lineage
                            .cuts()
                            .iter()
                            .rev()
                            .find_map(|(marker, cut)| cut.revert.is_some().then_some(*marker))
                            .unwrap_or(OplogIndex::NONE),
                    )
                    .map_err(|error| error.to_string())?;
            }
            OplogEntry::StreamEnd {
                entity_parent_start_index,
                record,
                ..
            } => {
                let record = oplog.download_payload(owner, mode, record.clone()).await?;
                let stream_id = qualify_local_stream(
                    record.stream_id,
                    owner.environment_id,
                    &owner.agent_id,
                    fingerprint,
                )
                .map_err(|error| error.to_string())?;
                projection.stream(stream_id).await?;
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
                let stream_id = qualify_local_stream(
                    record.stream_id,
                    owner.environment_id,
                    &owner.agent_id,
                    fingerprint,
                )
                .map_err(|error| error.to_string())?;
                projection.stream(stream_id).await?;
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
                if let Some(original_index) = record.reobserves() {
                    if original_index >= *index {
                        return Err(
                            "consumer re-observation must reference an earlier record".into()
                        );
                    }
                    let original = oplog
                        .read_exact(owner, mode, original_index, 1)
                        .await
                        .map_err(|error| error.to_string())?;
                    let Some(OplogEntry::StreamSession {
                        record: original, ..
                    }) = original.get(&original_index)
                    else {
                        return Err(
                            "consumer re-observation has no original consumer record".into()
                        );
                    };
                    let original = oplog
                        .download_payload(owner, mode, original.clone())
                        .await?;
                    if !record.has_supported_format() || !record.matches_reobserved(&original) {
                        return Err(
                            "consumer re-observation differs from its original input".into()
                        );
                    }
                }
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
                    fork = Some(projection.fork_cut(cut, owner, fingerprint).await?);
                    continue;
                }
                if lineage.resets_control(*index, &record) {
                    continue;
                }
                projection.session(&record, owner, fingerprint).await?;
                projection
                    .index
                    .apply_session_references(
                        *entity_parent_start_index,
                        &record,
                        owner.environment_id,
                        &owner.agent_id,
                        fingerprint,
                    )
                    .map_err(|error| error.to_string())?;
                projection.index.apply_result_offset(
                    *index,
                    &record,
                    owner.environment_id,
                    &owner.agent_id,
                    fingerprint,
                );
                if let StreamSessionRecord::ExternalProducerState(value) = &record {
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
                for key in ProducerMetadataKey::session_record(
                    &record,
                    owner.environment_id,
                    &owner.agent_id,
                    fingerprint,
                ) {
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
                        .apply_finished(record, owner.environment_id, &owner.agent_id, fingerprint)
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
                producer_fingerprint,
                vec![ProducerMetadataKey::Global, ProducerMetadataKey::Lineage],
            )
            .await
            .map_err(StreamStoreError::from)?;
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
        producer.set_control_metadata_provider(service, mode, producer_fingerprint);
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
                    .filter_map(|(source, _)| match source {
                        StreamRecordReference::Local(local_id) => qualify_local_stream(
                            *local_id,
                            self.environment_id,
                            &self.producer,
                            self.producer_fingerprint,
                        )
                        .ok()
                        .map(ProducerMetadataKey::Stream),
                        StreamRecordReference::Foreign(_) => None,
                    }),
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
        let Some((service, mode, fingerprint)) = self.control_metadata_provider.get() else {
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
                .lookup_durable_stream_producer_metadata(&owner, *mode, *fingerprint, batch.clone())
                .await
                .map_err(StreamStoreError::from)?;
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
        let Some((service, mode, fingerprint)) = self.control_metadata_provider.get() else {
            return Ok(None);
        };
        let _guard = self.index.lock().await;
        let owner = OwnedAgentId::new(self.environment_id, &self.producer);
        let (_, mut rows) = service
            .lookup_durable_stream_producer_metadata(
                &owner,
                *mode,
                *fingerprint,
                vec![ProducerMetadataKey::Global],
            )
            .await
            .map_err(StreamStoreError::from)?;
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
                *fingerprint,
                pages
                    .iter()
                    .map(|n| ProducerMetadataKey::AttachmentPage(*n))
                    .collect(),
            )
            .await
            .map_err(StreamStoreError::from)?;
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
            .lookup_durable_stream_producer_metadata(&owner, *mode, *fingerprint, keys)
            .await
            .map_err(StreamStoreError::from)?;
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
                attachment: attachment.as_ref().clone(),
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
        let owns_handle = self.owns_handle_identity(handle);
        if !owns_handle {
            keys.push(ProducerMetadataKey::Lineage);
        }
        if let Some(after) = after {
            keys.push(ProducerMetadataKey::Position(
                handle.stream_id,
                after.producer_oplog_index(),
            ));
        }
        let (stream, producer_generation, position) = if owns_handle {
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
            (stream, self.generation(), position)
        } else {
            let (service, _, _) = self.control_metadata_provider.get().ok_or_else(|| {
                StreamStoreError::Oplog("producer metadata service is unavailable".into())
            })?;
            let owner = OwnedAgentId::new(handle.producer_environment_id, &handle.producer);
            let identity = service
                .resolve_agent_identity(&owner)
                .await
                .map_err(|err| StreamStoreError::Oplog(err.to_string()))?
                .ok_or(StreamStoreError::InvalidHandle)?;
            if identity.fingerprint != handle.expected_producer_fingerprint {
                crate::metrics::workers::record_foreign_stream_fingerprint_mismatch();
                return Err(StreamStoreError::InvalidHandle);
            }
            let (_, rows) = service
                .lookup_durable_stream_producer_metadata(
                    &owner,
                    identity.agent_mode,
                    handle.expected_producer_fingerprint,
                    keys,
                )
                .await
                .map_err(StreamStoreError::from)?;
            let mut rows = rows.into_iter();
            let Some(Some(ProducerMetadataRow::Stream(stream))) = rows.next() else {
                return Err(StreamStoreError::InvalidHandle);
            };
            let Some(Some(ProducerMetadataRow::Lineage(lineage))) = rows.next() else {
                return Err(StreamStoreError::InvalidHandle);
            };
            let producer_generation = lineage
                .cuts()
                .iter()
                .rev()
                .find_map(|(marker, cut)| cut.revert.is_some().then_some(*marker))
                .unwrap_or(OplogIndex::NONE);
            let position = match rows.next().flatten() {
                Some(ProducerMetadataRow::Position(first, count)) => Some((first, count)),
                _ => None,
            };
            (stream, producer_generation, position)
        };
        if !stream.registration.accepts(handle, producer_generation) {
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
        let owner = OwnedAgentId::new(self.environment_id, &self.producer);
        let event = read_terminal_event(
            self.oplog.as_ref(),
            &self.fork_lineage,
            &owner,
            self.producer_fingerprint,
            stream_id,
            offset,
        )
        .await?;
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
    _lineage: &StreamForkLineage,
    owner: &OwnedAgentId,
    owner_fingerprint: AgentFingerprint,
    stream_id: StreamId,
    offset: StreamOffset,
) -> Result<CommittedProducerStreamEvent, StreamStoreError> {
    let (id, sequence, recorded_offset, terminal_author, payload) =
        match oplog.read(offset.producer_oplog_index()).await? {
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
    let id = qualify_local_stream(id, owner.environment_id, &owner.agent_id, owner_fingerprint)?;
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
        terminal_author: Some(terminal_author),
        nested_handles: Vec::new(),
        nested_references: Vec::new(),
        payload,
    })
}

#[cfg(test)]
mod tests;
