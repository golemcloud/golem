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
#[cfg(test)]
use crate::storage::keyvalue::KeyValueStorage;
#[cfg(test)]
use golem_common::model::durable_stream::StreamForkCutRecord;
use golem_common::serialization::serialize;

mod indexed;
mod projection;
mod rows;

pub(super) use indexed::read_terminal_event;
#[allow(unused_imports)]
pub(crate) use projection::ForkControlProjection;
pub(crate) use projection::{ProducerMetadataProjection, project_producer_metadata};

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

#[cfg(test)]
mod tests;
