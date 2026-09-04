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

use crate::durable_host::stream_bus::{
    DurableLiveStreamBus, DurableLiveStreamBusError, DurableLiveStreamEvent,
    DurableLiveStreamSubscription,
};
#[cfg(test)]
use crate::services::oplog::CommitLevel;
use crate::services::oplog::{
    DurableStreamOplogRecord, Oplog, OplogOps, OplogService, OplogServiceOps,
};
use crate::services::rpc::Rpc;
use crate::services::worker::WorkerService;
use async_trait::async_trait;
use golem_common::base_model::component::ComponentRevision;
use golem_common::base_model::durable_stream::{
    AttachedStreamSegmentRequestV1, AttachmentId, DEFAULT_LIVE_JOIN_BUFFER_SIZE,
    DURABLE_STREAM_FORMAT_VERSION, DurableStreamHandleV1, InputStreamHighWaterV1,
    MAX_DURABLE_STREAM_ITEM_SIZE, MAX_DURABLE_STREAMS_PER_SESSION,
    MAX_NEW_STREAM_HANDLES_PER_VALUE, MAX_PACKED_U8_STREAM_ITEM_SIZE,
    MAX_STREAM_VALUE_TRAVERSAL_DEPTH, STREAM_ATTACHMENT_LEASE_TTL_MILLIS, SessionStreamRoleV1,
    StreamAttachmentActivatedRecordV1, StreamAttachmentControlOperationV1,
    StreamAttachmentControlRequestV1, StreamAttachmentFinalizationReasonV1,
    StreamAttachmentFinalizedRecordV1, StreamAttachmentKeyV1, StreamAttachmentPreparedRecordV1,
    StreamAttachmentRenewedRecordV1, StreamCancelReasonV1, StreamCancelRecordV1,
    StreamCancelRoleV1, StreamCascadeDependentResultV1, StreamCascadeOutboxRecordV1,
    StreamEndRecordV1, StreamEndResultV1, StreamId, StreamInvocationIdV1, StreamItemsPayloadV1,
    StreamItemsRecordV1, StreamOffsetV1, StreamProducerDeletingRecordV1, StreamRegisteredRecordV1,
    StreamRegistrationCoordinateV1, StreamSessionAttachedRecordV1, StreamSessionFinishedRecordV1,
    StreamSessionInputHighWaterRecordV1, StreamSessionKeyV1, StreamSessionMappingRecordV1,
    StreamSessionMappingV1, StreamSessionPreparedRecordV1, StreamSessionRecordV1,
    StreamSourceKindV1, StreamSourceUnavailableRecordV1, StreamTerminalAuthorV1,
};
use golem_common::base_model::environment::EnvironmentId;
use golem_common::base_model::oplog::OplogEntry;
use golem_common::base_model::{AgentFingerprint, AgentId, OplogIndex};
use golem_common::model::OwnedAgentId;
use golem_common::model::agent::{AgentError, AgentMode};
use golem_common::model::oplog::payload::OplogPayload;
use golem_schema::schema::{
    SchemaFingerprintV1, SchemaGraph, SchemaType, SchemaValue, TypedSchemaValue,
};
use golem_service_base::model::auth::AuthCtx;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use tokio::sync::{Mutex, MutexGuard, Notify, oneshot};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProducerRegistrationRequestV1 {
    pub(crate) coordinate: StreamRegistrationCoordinateV1,
    pub(crate) source_invocation: StreamInvocationIdV1,
    pub(crate) component_revision: ComponentRevision,
    pub(crate) element_schema_fingerprint: SchemaFingerprintV1,
    pub(crate) source_kind: StreamSourceKindV1,
    pub(crate) session_mapping: Option<StreamSessionMappingV1>,
}

#[derive(Clone)]
pub(crate) enum NestedStreamWriteV1 {
    Register(ProducerRegistrationRequestV1),
    Forward(DurableStreamHandleV1),
}

#[derive(Clone, Debug, Eq, PartialEq, desert_rust::BinaryCodec)]
pub(crate) enum CommittedProducerStreamEventPayloadV1 {
    Value(Vec<u8>),
    PackedU8(u8),
    End(StreamEndResultV1),
    Cancel {
        role: StreamCancelRoleV1,
        reason: StreamCancelReasonV1,
        details: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, desert_rust::BinaryCodec)]
pub(crate) struct CommittedProducerStreamEventV1 {
    pub(crate) stream_id: StreamId,
    pub(crate) producer_sequence: u64,
    pub(crate) offset: StreamOffsetV1,
    pub(crate) packed_u8_batch_end: Option<StreamOffsetV1>,
    pub(crate) terminal_author: Option<StreamTerminalAuthorV1>,
    pub(crate) nested_handles: Vec<DurableStreamHandleV1>,
    pub(crate) payload: CommittedProducerStreamEventPayloadV1,
}

impl CommittedProducerStreamEventV1 {
    pub(crate) fn is_terminal(&self) -> bool {
        matches!(
            self.payload,
            CommittedProducerStreamEventPayloadV1::End(_)
                | CommittedProducerStreamEventPayloadV1::Cancel { .. }
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProducerWriteOutcomeV1<T> {
    pub(crate) value: T,
    pub(crate) replayed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DurableStreamProducerError {
    UnsupportedVersion(u8),
    InvalidHandle,
    RegistrationDivergence,
    UnknownStream(StreamId),
    AlreadyTerminal(StreamId),
    FencedByTerminal(CommittedProducerStreamEventPayloadV1),
    EventConflict,
    SequenceGap { expected: u64, actual: u64 },
    CounterOverflow,
    ItemTooLarge,
    InvalidPackedU8Batch,
    InvalidValueBatch,
    StreamLimit,
    ValueStreamLimit,
    TraversalDepthLimit,
    InvalidOffset(String),
    CursorUnavailable,
    SessionFinished(StreamSessionKeyV1),
    AttachmentConflict,
    StaleEpoch { current: u64, actual: u64 },
    InvalidEpoch { current: u64, actual: u64 },
    InvalidAttachmentState,
    LeaseExpired,
    ProducerDeleting,
    ConsumerDeleting,
    ConsumerJournalAdvanced,
    DeletionBlocked(Vec<StreamAttachmentKeyV1>),
    CorruptHistory(String),
    Oplog(String),
    LiveBus(DurableLiveStreamBusError),
}

impl std::fmt::Display for DurableStreamProducerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for DurableStreamProducerError {}

impl DurableStreamProducerError {
    pub(crate) fn deletion_blocked_evidence(&self) -> Option<String> {
        let Self::DeletionBlocked(dependents) = self else {
            return None;
        };
        Some(
            dependents
                .iter()
                .map(|key| {
                    format!(
                        "attachment={}, stream={}, epoch={}, consumer={}/{}, consumer_fingerprint={}",
                        key.attachment_id.0,
                        key.stream_id.0,
                        key.epoch,
                        key.consumer_environment_id,
                        key.consumer,
                        key.expected_consumer_fingerprint.0,
                    )
                })
                .collect::<Vec<_>>()
                .join("; "),
        )
    }
}

impl From<DurableLiveStreamBusError> for DurableStreamProducerError {
    fn from(value: DurableLiveStreamBusError) -> Self {
        Self::LiveBus(value)
    }
}

#[derive(Clone, Default)]
struct ProducerStreamIndex {
    registrations: HashMap<StreamId, StreamRegisteredRecordV1>,
    referenced_handles: HashMap<StreamId, (DurableStreamHandleV1, HashSet<StreamSessionKeyV1>)>,
    coordinates: HashMap<StreamRegistrationCoordinateV1, StreamId>,
    streams: HashMap<StreamId, IndexedProducerStream>,
    stream_sessions: HashMap<StreamId, StreamSessionKeyV1>,
    stream_roles: HashMap<StreamId, SessionStreamRoleV1>,
    session_stream_mappings:
        HashMap<StreamSessionKeyV1, HashSet<(DurableStreamHandleV1, SessionStreamRoleV1)>>,
    session_stream_counts: HashMap<StreamSessionKeyV1, usize>,
    finished_sessions: HashSet<StreamSessionKeyV1>,
    attachments: HashMap<(AttachmentId, StreamId), IndexedStreamAttachment>,
    cascade_outbox: HashMap<StreamAttachmentKeyV1, StreamCascadeDependentResultV1>,
    consumer_journals: HashMap<(StreamSessionKeyV1, StreamId), IndexedConsumerJournal>,
    deleting: bool,
    consumer_deleting: bool,
}

#[derive(Clone, Default)]
struct IndexedConsumerJournal {
    next_read_ordinal: u64,
    terminal: bool,
    source_unavailable: Option<(StreamAttachmentKeyV1, StreamOffsetV1)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum IndexedStreamAttachmentState {
    Prepared {
        prepared_at_millis: u64,
        lease_expires_at_millis: u64,
    },
    Active {
        activated_at_millis: u64,
        lease_expires_at_millis: u64,
    },
    Finalized {
        finalized_at_millis: u64,
        reason: StreamAttachmentFinalizationReasonV1,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct IndexedStreamAttachment {
    key: StreamAttachmentKeyV1,
    state: IndexedStreamAttachmentState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum StreamAttachmentStateV1 {
    Prepared,
    Active,
    Finalized(StreamAttachmentFinalizationReasonV1),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StreamAttachmentViewV1 {
    pub(crate) key: StreamAttachmentKeyV1,
    pub(crate) state: StreamAttachmentStateV1,
    pub(crate) lease_expires_at_millis: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StreamDeletionDiagnosticsV1 {
    pub(crate) deleting: bool,
    pub(crate) attachments: Vec<StreamAttachmentViewV1>,
    pub(crate) cascade_completed: Vec<(StreamAttachmentKeyV1, StreamCascadeDependentResultV1)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AttachmentApplyOutcome {
    Changed,
    Replayed,
}

#[derive(Clone, Default)]
struct IndexedProducerStream {
    events: BTreeMap<u64, CommittedProducerStreamEventV1>,
    batches: BTreeMap<u64, (StreamItemsPayloadV1, Vec<StreamOffsetV1>, Vec<StreamId>)>,
    next_sequence: u64,
    terminal: bool,
}

impl ProducerStreamIndex {
    fn ensure_producer_write_allowed(&self) -> Result<(), DurableStreamProducerError> {
        if self.deleting {
            Err(DurableStreamProducerError::ProducerDeleting)
        } else {
            Ok(())
        }
    }

    fn apply_deletion_record(
        &mut self,
        record: &StreamSessionRecordV1,
        environment_id: EnvironmentId,
        producer: &AgentId,
        producer_fingerprint: AgentFingerprint,
    ) -> Result<(), DurableStreamProducerError> {
        match record {
            StreamSessionRecordV1::ProducerDeleting(record) => {
                if record.producer_environment_id != environment_id
                    || record.producer != *producer
                    || record.producer_fingerprint != producer_fingerprint
                {
                    return Err(DurableStreamProducerError::CorruptHistory(
                        "durable stream deletion barrier identifies another producer incarnation"
                            .to_string(),
                    ));
                }
                self.deleting = true;
            }
            StreamSessionRecordV1::ConsumerDeleting(record) => {
                if record.consumer_environment_id != environment_id
                    || record.consumer != *producer
                    || record.consumer_fingerprint != producer_fingerprint
                {
                    return Err(DurableStreamProducerError::CorruptHistory(
                        "durable stream consumer deletion intent identifies another consumer incarnation"
                            .to_string(),
                    ));
                }
                self.consumer_deleting = true;
            }
            StreamSessionRecordV1::CascadeOutbox(record) => {
                self.validate_attachment_key(
                    &record.key,
                    environment_id,
                    producer,
                    producer_fingerprint,
                )?;
                match self.cascade_outbox.get(&record.key) {
                    Some(existing) if existing != &record.result => {
                        return Err(DurableStreamProducerError::CorruptHistory(
                            "conflicting durable stream cascade outbox result".to_string(),
                        ));
                    }
                    Some(_) => {}
                    None => {
                        self.cascade_outbox
                            .insert(record.key.clone(), record.result.clone());
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn apply_session_references(
        &mut self,
        record: &StreamSessionRecordV1,
    ) -> Result<(), DurableStreamProducerError> {
        if self.consumer_deleting
            && matches!(
                record,
                StreamSessionRecordV1::TopologyPrepared(_)
                    | StreamSessionRecordV1::TopologyActivated(_)
            )
        {
            return Err(DurableStreamProducerError::ConsumerDeleting);
        }
        self.apply_consumer_journal_record(record)?;
        let (session_key, mappings): (&StreamSessionKeyV1, &[StreamSessionMappingRecordV1]) =
            match record {
                StreamSessionRecordV1::Mapping(record) => {
                    (&record.session_key, std::slice::from_ref(&record.mapping))
                }
                StreamSessionRecordV1::InvocationResult(record) => {
                    (&record.session_key, &record.stream_mappings)
                }
                StreamSessionRecordV1::Prepared(record) => {
                    (&record.attempt.session_key, &record.stream_mappings)
                }
                _ => return Ok(()),
            };
        if matches!(
            record,
            StreamSessionRecordV1::Prepared(_) | StreamSessionRecordV1::InvocationResult(_)
        ) && mappings.len() > MAX_NEW_STREAM_HANDLES_PER_VALUE
        {
            return Err(DurableStreamProducerError::ValueStreamLimit);
        }
        let existing_mappings = self.session_stream_mappings.get(session_key);
        let mut new_mappings = HashSet::new();
        for mapping in mappings {
            if mapping.handle.format_version != DURABLE_STREAM_FORMAT_VERSION {
                return Err(DurableStreamProducerError::InvalidHandle);
            }
            if let Some((existing, _)) = self.referenced_handles.get(&mapping.handle.stream_id)
                && existing != &mapping.handle
            {
                return Err(DurableStreamProducerError::CorruptHistory(
                    "durable stream mapping relabels an existing stream handle".to_string(),
                ));
            }
            let identity = (mapping.handle.clone(), mapping.role);
            if existing_mappings.is_none_or(|existing| !existing.contains(&identity)) {
                new_mappings.insert(identity);
            }
        }
        let current = self
            .session_stream_counts
            .get(session_key)
            .copied()
            .unwrap_or_default();
        if current
            .checked_add(new_mappings.len())
            .is_none_or(|count| count > MAX_DURABLE_STREAMS_PER_SESSION)
        {
            return Err(DurableStreamProducerError::StreamLimit);
        }
        let new_mapping_count = new_mappings.len();
        for mapping in mappings {
            match self.referenced_handles.get_mut(&mapping.handle.stream_id) {
                Some((existing, _)) if existing != &mapping.handle => {
                    unreachable!("stream handle conflict was validated before index mutation")
                }
                Some((_, sessions)) => {
                    sessions.insert(session_key.clone());
                }
                None => {
                    self.referenced_handles.insert(
                        mapping.handle.stream_id,
                        (mapping.handle.clone(), HashSet::from([session_key.clone()])),
                    );
                }
            }
        }
        self.session_stream_mappings
            .entry(session_key.clone())
            .or_default()
            .extend(new_mappings);
        self.session_stream_counts.insert(
            session_key.clone(),
            current
                .checked_add(new_mapping_count)
                .expect("validated durable session stream count cannot overflow"),
        );
        Ok(())
    }

    fn apply_consumer_journal_record(
        &mut self,
        record: &StreamSessionRecordV1,
    ) -> Result<(), DurableStreamProducerError> {
        let (session_key, stream_id, ordinal, item_count, terminal) = match record {
            StreamSessionRecordV1::ConsumerItemValue(record) => (
                &record.session_key,
                record.stream_id,
                record.consumer_read_ordinal,
                record.logical_item_count(),
                false,
            ),
            StreamSessionRecordV1::ConsumerTerminal(record) => (
                &record.session_key,
                record.stream_id,
                record.consumer_read_ordinal,
                1,
                true,
            ),
            StreamSessionRecordV1::SourceUnavailable(record) => {
                let journal = self
                    .consumer_journals
                    .entry((record.key.session_key.clone(), record.key.stream_id))
                    .or_default();
                if let Some((existing_key, existing_offset)) = &journal.source_unavailable {
                    return if existing_key == &record.key
                        && existing_offset == &record.source_offset
                    {
                        Ok(())
                    } else {
                        Err(DurableStreamProducerError::AttachmentConflict)
                    };
                }
                if journal.terminal || record.consumer_read_ordinal != journal.next_read_ordinal {
                    return Err(DurableStreamProducerError::ConsumerJournalAdvanced);
                }
                journal.source_unavailable = Some((record.key.clone(), record.source_offset));
                return Ok(());
            }
            _ => return Ok(()),
        };
        let journal = self
            .consumer_journals
            .entry((session_key.clone(), stream_id))
            .or_default();
        if journal.terminal
            || journal.source_unavailable.is_some()
            || ordinal != journal.next_read_ordinal
        {
            return Err(DurableStreamProducerError::ConsumerJournalAdvanced);
        }
        journal.next_read_ordinal = journal
            .next_read_ordinal
            .checked_add(
                u64::try_from(item_count)
                    .map_err(|_| DurableStreamProducerError::CounterOverflow)?,
            )
            .ok_or(DurableStreamProducerError::CounterOverflow)?;
        journal.terminal = terminal;
        Ok(())
    }

    fn registration_session_key(
        &self,
        coordinate: &StreamRegistrationCoordinateV1,
        mapping: &Option<StreamSessionMappingV1>,
    ) -> Option<StreamSessionKeyV1> {
        if let Some(mapping) = mapping {
            return Some(mapping.session_key.clone());
        }
        match coordinate {
            StreamRegistrationCoordinateV1::Root { invocation_id, .. } => {
                Some(invocation_id.clone())
            }
            StreamRegistrationCoordinateV1::Nested {
                parent_stream_id, ..
            } => self.stream_sessions.get(parent_stream_id).cloned(),
        }
    }

    fn apply_registration(
        &mut self,
        oplog_index: OplogIndex,
        record: StreamRegisteredRecordV1,
        environment_id: EnvironmentId,
        producer: &AgentId,
        producer_fingerprint: AgentFingerprint,
    ) -> Result<(), DurableStreamProducerError> {
        validate_version(record.format_version)?;
        if registration_coordinate_depth(&record.coordinate) > MAX_STREAM_VALUE_TRAVERSAL_DEPTH {
            return Err(DurableStreamProducerError::CorruptHistory(
                "stream registration coordinate exceeds the traversal-depth limit".to_string(),
            ));
        }
        if record.registration_oplog_index != oplog_index
            || record.handle.format_version != DURABLE_STREAM_FORMAT_VERSION
            || record.handle.stream_id
                != StreamId::derive(environment_id, producer, producer_fingerprint, oplog_index)
                    .map_err(|error| {
                        DurableStreamProducerError::CorruptHistory(error.to_string())
                    })?
            || record.handle.producer_environment_id != environment_id
            || record.handle.producer != *producer
            || record.handle.expected_producer_fingerprint != producer_fingerprint
        {
            return Err(DurableStreamProducerError::InvalidHandle);
        }
        if let Some(existing_id) = self.coordinates.get(&record.coordinate) {
            let existing = self
                .registrations
                .get(existing_id)
                .expect("coordinate index points at a missing stream registration");
            return if existing == &record {
                Ok(())
            } else {
                Err(DurableStreamProducerError::RegistrationDivergence)
            };
        }
        if self.registrations.contains_key(&record.handle.stream_id) {
            return Err(DurableStreamProducerError::RegistrationDivergence);
        }
        let session_key = self
            .registration_session_key(&record.coordinate, &record.session_mapping)
            .ok_or_else(|| {
                DurableStreamProducerError::CorruptHistory(
                    "nested stream registration references an unknown parent stream".to_string(),
                )
            })?;
        if self.finished_sessions.contains(&session_key) {
            return Err(DurableStreamProducerError::CorruptHistory(
                "stream registration follows its session Finished record".to_string(),
            ));
        }
        let role = match (&record.coordinate, &record.session_mapping) {
            (_, Some(mapping)) => mapping.role,
            (
                StreamRegistrationCoordinateV1::Nested {
                    parent_stream_id, ..
                },
                None,
            ) => *self.stream_roles.get(parent_stream_id).ok_or_else(|| {
                DurableStreamProducerError::CorruptHistory(
                    "nested stream registration references a parent without a session role"
                        .to_string(),
                )
            })?,
            (
                StreamRegistrationCoordinateV1::Root {
                    root_kind:
                        golem_common::base_model::durable_stream::StreamRootKindV1::MethodInput,
                    ..
                },
                None,
            ) => SessionStreamRoleV1::Input,
            (
                StreamRegistrationCoordinateV1::Root {
                    root_kind:
                        golem_common::base_model::durable_stream::StreamRootKindV1::MethodResult,
                    ..
                },
                None,
            ) => SessionStreamRoleV1::Output,
        };
        let mapping_identity = (record.handle.clone(), role);
        let new_session_mapping = self
            .session_stream_mappings
            .get(&session_key)
            .is_none_or(|mappings| !mappings.contains(&mapping_identity));
        if new_session_mapping
            && self
                .session_stream_counts
                .get(&session_key)
                .copied()
                .unwrap_or_default()
                >= MAX_DURABLE_STREAMS_PER_SESSION
        {
            return Err(DurableStreamProducerError::StreamLimit);
        }
        self.coordinates
            .insert(record.coordinate.clone(), record.handle.stream_id);
        self.streams
            .insert(record.handle.stream_id, IndexedProducerStream::default());
        self.stream_sessions
            .insert(record.handle.stream_id, session_key.clone());
        self.stream_roles.insert(record.handle.stream_id, role);
        if new_session_mapping {
            self.session_stream_mappings
                .entry(session_key.clone())
                .or_default()
                .insert(mapping_identity);
            *self.session_stream_counts.entry(session_key).or_default() += 1;
        }
        self.registrations.insert(record.handle.stream_id, record);
        Ok(())
    }

    fn apply_item_batch(
        &mut self,
        oplog_index: OplogIndex,
        pending_registrations: Vec<(OplogIndex, StreamRegisteredRecordV1)>,
        record: StreamItemsRecordV1,
        environment_id: EnvironmentId,
        producer: &AgentId,
        producer_fingerprint: AgentFingerprint,
    ) -> Result<Vec<CommittedProducerStreamEventV1>, DurableStreamProducerError> {
        if pending_registrations.len() != record.newly_registered_stream_ids.len() {
            return Err(DurableStreamProducerError::CorruptHistory(
                "nested registrations are not claimed by their enclosing item batch".to_string(),
            ));
        }
        let registration_start = oplog_index
            .as_u64()
            .checked_sub(pending_registrations.len() as u64)
            .ok_or_else(|| {
                DurableStreamProducerError::CorruptHistory(
                    "nested registrations precede the beginning of the oplog".to_string(),
                )
            })?;
        let logical_item_count = record.payload.logical_item_count() as u64;
        for (position, ((registration_index, registration), expected_stream_id)) in
            pending_registrations
                .iter()
                .zip(&record.newly_registered_stream_ids)
                .enumerate()
        {
            let expected_index = OplogIndex::from_u64(registration_start + position as u64);
            if *registration_index != expected_index
                || registration.handle.stream_id != *expected_stream_id
                || !nested_coordinate_matches_item(
                    &registration.coordinate,
                    record.stream_id,
                    record.first_sequence,
                    logical_item_count,
                )
            {
                return Err(DurableStreamProducerError::CorruptHistory(
                    "nested registration batch does not match its enclosing stream item"
                        .to_string(),
                ));
            }
        }
        let mut updated = self.clone();
        for (registration_index, registration) in pending_registrations {
            updated.apply_registration(
                registration_index,
                registration,
                environment_id,
                producer,
                producer_fingerprint,
            )?;
        }
        let events = updated.apply_items(oplog_index, record, producer_fingerprint)?;
        *self = updated;
        Ok(events)
    }

    fn apply_items(
        &mut self,
        oplog_index: OplogIndex,
        record: StreamItemsRecordV1,
        producer_fingerprint: AgentFingerprint,
    ) -> Result<Vec<CommittedProducerStreamEventV1>, DurableStreamProducerError> {
        validate_version(record.format_version)?;
        if record.producer_fingerprint != producer_fingerprint {
            return Err(DurableStreamProducerError::InvalidHandle);
        }
        validate_items_payload(&record.payload)?;
        let session_key = self
            .stream_sessions
            .get(&record.stream_id)
            .ok_or(DurableStreamProducerError::UnknownStream(record.stream_id))?;
        if self.finished_sessions.contains(session_key) {
            return Err(DurableStreamProducerError::CorruptHistory(
                "stream item follows its session Finished record".to_string(),
            ));
        }
        let logical_item_count = record.payload.logical_item_count() as u64;
        if matches!(record.payload, StreamItemsPayloadV1::PackedU8(_))
            && !record.nested_stream_ids.is_empty()
        {
            return Err(DurableStreamProducerError::CorruptHistory(
                "packed-u8 stream items cannot contain nested streams".to_string(),
            ));
        }
        for nested_stream_id in &record.nested_stream_ids {
            if let Some(registration) = self.registrations.get(nested_stream_id) {
                if !nested_coordinate_matches_item(
                    &registration.coordinate,
                    record.stream_id,
                    record.first_sequence,
                    logical_item_count,
                ) && self.referenced_handles.get(nested_stream_id).is_none_or(
                    |(_, referenced_sessions)| !referenced_sessions.contains(session_key),
                ) {
                    return Err(DurableStreamProducerError::CorruptHistory(
                        "nested stream registration does not identify its enclosing stream item"
                            .to_string(),
                    ));
                }
            } else if self
                .referenced_handles
                .get(nested_stream_id)
                .is_none_or(|(_, referenced_sessions)| !referenced_sessions.contains(session_key))
            {
                return Err(DurableStreamProducerError::CorruptHistory(
                    "stream item batch references an unknown durable stream handle".to_string(),
                ));
            }
        }
        let nested_handles = record
            .nested_stream_ids
            .iter()
            .map(|stream_id| {
                self.registrations
                    .get(stream_id)
                    .map(|registration| registration.handle.clone())
                    .or_else(|| {
                        self.referenced_handles
                            .get(stream_id)
                            .map(|(handle, _)| handle.clone())
                    })
                    .expect("validated nested durable stream handle is missing")
            })
            .collect::<Vec<_>>();
        let registration_start = oplog_index
            .as_u64()
            .checked_sub(record.newly_registered_stream_ids.len() as u64)
            .ok_or_else(|| {
                DurableStreamProducerError::CorruptHistory(
                    "nested registrations precede the beginning of the oplog".to_string(),
                )
            })?;
        let nested_ids: HashSet<_> = record.nested_stream_ids.iter().copied().collect();
        if nested_ids.len() != record.nested_stream_ids.len() {
            return Err(DurableStreamProducerError::CorruptHistory(
                "stream item batch contains duplicate nested stream ownership".to_string(),
            ));
        }
        let mut newly_registered_ids =
            HashSet::with_capacity(record.newly_registered_stream_ids.len());
        for (position, stream_id) in record.newly_registered_stream_ids.iter().enumerate() {
            if !newly_registered_ids.insert(*stream_id) || !nested_ids.contains(stream_id) {
                return Err(DurableStreamProducerError::CorruptHistory(
                    "stream item batch has an invalid newly registered stream list".to_string(),
                ));
            }
            let expected_index = OplogIndex::from_u64(registration_start + position as u64);
            if self
                .registrations
                .get(stream_id)
                .is_none_or(|registration| registration.registration_oplog_index != expected_index)
            {
                return Err(DurableStreamProducerError::CorruptHistory(
                    "stream item batch does not follow its declared nested registrations"
                        .to_string(),
                ));
            }
        }
        let stream = self
            .streams
            .get_mut(&record.stream_id)
            .ok_or(DurableStreamProducerError::UnknownStream(record.stream_id))?;
        if stream.terminal {
            return Err(DurableStreamProducerError::AlreadyTerminal(
                record.stream_id,
            ));
        }
        if record.first_sequence != stream.next_sequence {
            return Err(DurableStreamProducerError::SequenceGap {
                expected: stream.next_sequence,
                actual: record.first_sequence,
            });
        }
        let payloads = logical_payloads(&record.payload);
        if payloads.len() != record.offsets.len() {
            return Err(DurableStreamProducerError::CorruptHistory(
                "item count does not match offset count".to_string(),
            ));
        }
        let packed_u8_batch_end = if matches!(&record.payload, StreamItemsPayloadV1::PackedU8(_)) {
            record.offsets.last().copied()
        } else {
            None
        };
        let mut events = Vec::with_capacity(payloads.len());
        for (sub_index, (payload, offset)) in payloads
            .into_iter()
            .zip(record.offsets.iter().copied())
            .enumerate()
        {
            if offset != StreamOffsetV1::new(oplog_index, sub_index as u32) {
                return Err(DurableStreamProducerError::CorruptHistory(
                    "stream item offset does not match its producer oplog position".to_string(),
                ));
            }
            let sequence = record
                .first_sequence
                .checked_add(sub_index as u64)
                .ok_or(DurableStreamProducerError::CounterOverflow)?;
            let event = CommittedProducerStreamEventV1 {
                stream_id: record.stream_id,
                producer_sequence: sequence,
                offset,
                packed_u8_batch_end,
                terminal_author: None,
                nested_handles: nested_handles.clone(),
                payload,
            };
            stream.events.insert(sequence, event.clone());
            events.push(event);
        }
        stream.next_sequence = stream
            .next_sequence
            .checked_add(events.len() as u64)
            .ok_or(DurableStreamProducerError::CounterOverflow)?;
        stream.batches.insert(
            record.first_sequence,
            (record.payload, record.offsets, record.nested_stream_ids),
        );
        Ok(events)
    }

    fn apply_end(
        &mut self,
        oplog_index: OplogIndex,
        record: StreamEndRecordV1,
        producer_fingerprint: AgentFingerprint,
    ) -> Result<CommittedProducerStreamEventV1, DurableStreamProducerError> {
        validate_version(record.format_version)?;
        if record.producer_fingerprint != producer_fingerprint
            || record.offset != StreamOffsetV1::new(oplog_index, 0)
        {
            return Err(DurableStreamProducerError::InvalidHandle);
        }
        let stream = self
            .streams
            .get_mut(&record.stream_id)
            .ok_or(DurableStreamProducerError::UnknownStream(record.stream_id))?;
        validate_terminal_sequence(stream, record.stream_id, record.sequence)?;
        let event = CommittedProducerStreamEventV1 {
            stream_id: record.stream_id,
            producer_sequence: record.sequence,
            offset: record.offset,
            packed_u8_batch_end: None,
            terminal_author: Some(record.authored_by),
            nested_handles: Vec::new(),
            payload: CommittedProducerStreamEventPayloadV1::End(record.result),
        };
        stream.events.insert(record.sequence, event.clone());
        stream.terminal = true;
        Ok(event)
    }

    fn apply_finished(
        &mut self,
        record: &StreamSessionFinishedRecordV1,
    ) -> Result<(), DurableStreamProducerError> {
        validate_version(record.format_version)?;
        if self.finished_sessions.contains(&record.session_key) {
            return Ok(());
        }
        if self.stream_sessions.iter().any(|(stream_id, session_key)| {
            session_key == &record.session_key
                && self
                    .streams
                    .get(stream_id)
                    .is_some_and(|stream| !stream.terminal)
        }) {
            return Err(DurableStreamProducerError::CorruptHistory(
                "session Finished record precedes a materialized stream terminal".to_string(),
            ));
        }
        self.finished_sessions.insert(record.session_key.clone());
        Ok(())
    }

    fn apply_cancel(
        &mut self,
        oplog_index: OplogIndex,
        record: StreamCancelRecordV1,
        producer_fingerprint: AgentFingerprint,
    ) -> Result<CommittedProducerStreamEventV1, DurableStreamProducerError> {
        validate_version(record.format_version)?;
        if record.producer_fingerprint != producer_fingerprint
            || record.offset != StreamOffsetV1::new(oplog_index, 0)
        {
            return Err(DurableStreamProducerError::InvalidHandle);
        }
        let stream = self
            .streams
            .get_mut(&record.stream_id)
            .ok_or(DurableStreamProducerError::UnknownStream(record.stream_id))?;
        validate_terminal_sequence(stream, record.stream_id, record.sequence)?;
        let event = CommittedProducerStreamEventV1 {
            stream_id: record.stream_id,
            producer_sequence: record.sequence,
            offset: record.offset,
            packed_u8_batch_end: None,
            terminal_author: Some(record.authored_by),
            nested_handles: Vec::new(),
            payload: CommittedProducerStreamEventPayloadV1::Cancel {
                role: record.role,
                reason: record.reason,
                details: record.details,
            },
        };
        stream.events.insert(record.sequence, event.clone());
        stream.terminal = true;
        Ok(event)
    }

    fn apply_attachment_record(
        &mut self,
        record: &StreamSessionRecordV1,
        environment_id: EnvironmentId,
        producer: &AgentId,
        producer_fingerprint: AgentFingerprint,
    ) -> Result<AttachmentApplyOutcome, DurableStreamProducerError> {
        let (key, state) = match record {
            StreamSessionRecordV1::AttachmentPrepared(record) => {
                validate_version(record.format_version)?;
                if record.lease_expires_at_millis <= record.prepared_at_millis {
                    return Err(DurableStreamProducerError::AttachmentConflict);
                }
                (
                    &record.key,
                    IndexedStreamAttachmentState::Prepared {
                        prepared_at_millis: record.prepared_at_millis,
                        lease_expires_at_millis: record.lease_expires_at_millis,
                    },
                )
            }
            StreamSessionRecordV1::AttachmentActivated(record) => {
                validate_version(record.format_version)?;
                if record.lease_expires_at_millis <= record.activated_at_millis {
                    return Err(DurableStreamProducerError::AttachmentConflict);
                }
                (
                    &record.key,
                    IndexedStreamAttachmentState::Active {
                        activated_at_millis: record.activated_at_millis,
                        lease_expires_at_millis: record.lease_expires_at_millis,
                    },
                )
            }
            StreamSessionRecordV1::AttachmentRenewed(record) => {
                validate_version(record.format_version)?;
                if record.lease_expires_at_millis <= record.renewed_at_millis {
                    return Err(DurableStreamProducerError::AttachmentConflict);
                }
                (
                    &record.key,
                    IndexedStreamAttachmentState::Active {
                        activated_at_millis: record.renewed_at_millis,
                        lease_expires_at_millis: record.lease_expires_at_millis,
                    },
                )
            }
            StreamSessionRecordV1::AttachmentFinalized(record) => {
                validate_version(record.format_version)?;
                (
                    &record.key,
                    IndexedStreamAttachmentState::Finalized {
                        finalized_at_millis: record.finalized_at_millis,
                        reason: record.reason,
                    },
                )
            }
            _ => return Ok(AttachmentApplyOutcome::Replayed),
        };
        self.validate_attachment_key(key, environment_id, producer, producer_fingerprint)?;
        let slot = (key.attachment_id, key.stream_id);
        let existing = self.attachments.get(&slot);

        if self.deleting
            && matches!(
                record,
                StreamSessionRecordV1::AttachmentPrepared(_)
                    | StreamSessionRecordV1::AttachmentActivated(_)
                    | StreamSessionRecordV1::AttachmentRenewed(_)
            )
        {
            return Err(DurableStreamProducerError::ProducerDeleting);
        }

        if matches!(record, StreamSessionRecordV1::AttachmentPrepared(_)) {
            return match existing {
                None if key.epoch == 1 => {
                    self.attachments.insert(
                        slot,
                        IndexedStreamAttachment {
                            key: key.clone(),
                            state,
                        },
                    );
                    Ok(AttachmentApplyOutcome::Changed)
                }
                None => Err(DurableStreamProducerError::InvalidEpoch {
                    current: 0,
                    actual: key.epoch,
                }),
                Some(existing) => {
                    if key.epoch == existing.key.epoch.checked_add(1).unwrap_or_default()
                        && matches!(existing.state, IndexedStreamAttachmentState::Active { .. })
                        && attachment_identity_matches_except_epoch(&existing.key, key)
                    {
                        self.attachments.insert(
                            slot,
                            IndexedStreamAttachment {
                                key: key.clone(),
                                state,
                            },
                        );
                        Ok(AttachmentApplyOutcome::Changed)
                    } else {
                        validate_attachment_epoch(existing, key)?;
                        match existing.state {
                            IndexedStreamAttachmentState::Prepared { .. }
                            | IndexedStreamAttachmentState::Active { .. }
                                if existing.key == *key =>
                            {
                                Ok(AttachmentApplyOutcome::Replayed)
                            }
                            _ => Err(DurableStreamProducerError::InvalidAttachmentState),
                        }
                    }
                }
            };
        }

        let existing = existing.ok_or(DurableStreamProducerError::InvalidAttachmentState)?;
        validate_attachment_epoch(existing, key)?;
        if existing.key != *key {
            return Err(DurableStreamProducerError::AttachmentConflict);
        }

        let outcome = match record {
            StreamSessionRecordV1::AttachmentActivated(_) => match existing.state {
                IndexedStreamAttachmentState::Prepared { .. } => AttachmentApplyOutcome::Changed,
                IndexedStreamAttachmentState::Active { .. } => AttachmentApplyOutcome::Replayed,
                IndexedStreamAttachmentState::Finalized { .. } => {
                    return Err(DurableStreamProducerError::InvalidAttachmentState);
                }
            },
            StreamSessionRecordV1::AttachmentRenewed(record) => match existing.state {
                IndexedStreamAttachmentState::Active {
                    lease_expires_at_millis,
                    ..
                } if record.lease_expires_at_millis > lease_expires_at_millis => {
                    AttachmentApplyOutcome::Changed
                }
                IndexedStreamAttachmentState::Active { .. } => AttachmentApplyOutcome::Replayed,
                _ => return Err(DurableStreamProducerError::InvalidAttachmentState),
            },
            StreamSessionRecordV1::AttachmentFinalized(_) => match existing.state {
                IndexedStreamAttachmentState::Prepared { .. }
                | IndexedStreamAttachmentState::Active { .. } => AttachmentApplyOutcome::Changed,
                IndexedStreamAttachmentState::Finalized { .. } => AttachmentApplyOutcome::Replayed,
            },
            _ => unreachable!("non-attachment records returned before state application"),
        };
        if outcome == AttachmentApplyOutcome::Changed {
            self.attachments.insert(
                slot,
                IndexedStreamAttachment {
                    key: key.clone(),
                    state,
                },
            );
        }
        Ok(outcome)
    }

    fn validate_attachment_key(
        &self,
        key: &StreamAttachmentKeyV1,
        environment_id: EnvironmentId,
        producer: &AgentId,
        producer_fingerprint: AgentFingerprint,
    ) -> Result<(), DurableStreamProducerError> {
        let registration = self
            .registrations
            .get(&key.stream_id)
            .ok_or(DurableStreamProducerError::UnknownStream(key.stream_id))?;
        if key.producer_environment_id != environment_id
            || key.producer != *producer
            || key.expected_producer_fingerprint != producer_fingerprint
            || registration.handle.producer_environment_id != key.producer_environment_id
            || registration.handle.producer != key.producer
            || registration.handle.expected_producer_fingerprint
                != key.expected_producer_fingerprint
            || key.consumer_invocation.callee_environment_id != key.consumer_environment_id
            || key.consumer_invocation.callee != key.consumer
            || key.consumer_invocation.callee_fingerprint != key.expected_consumer_fingerprint
            || AttachmentId::primary(
                key.session_key.callee_environment_id,
                &key.session_key.callee,
                &key.session_key.idempotency_key,
            )
            .map_err(|error| DurableStreamProducerError::CorruptHistory(error.to_string()))?
                != key.attachment_id
        {
            return Err(DurableStreamProducerError::InvalidHandle);
        }
        Ok(())
    }

    fn attachment_views(&self) -> Vec<StreamAttachmentViewV1> {
        let mut views = self
            .attachments
            .values()
            .map(|attachment| {
                let (state, lease_expires_at_millis) = match attachment.state {
                    IndexedStreamAttachmentState::Prepared {
                        lease_expires_at_millis,
                        ..
                    } => (
                        StreamAttachmentStateV1::Prepared,
                        Some(lease_expires_at_millis),
                    ),
                    IndexedStreamAttachmentState::Active {
                        lease_expires_at_millis,
                        ..
                    } => (
                        StreamAttachmentStateV1::Active,
                        Some(lease_expires_at_millis),
                    ),
                    IndexedStreamAttachmentState::Finalized { reason, .. } => {
                        (StreamAttachmentStateV1::Finalized(reason), None)
                    }
                };
                StreamAttachmentViewV1 {
                    key: attachment.key.clone(),
                    state,
                    lease_expires_at_millis,
                }
            })
            .collect::<Vec<_>>();
        views.sort_by_key(|view| (view.key.stream_id, view.key.attachment_id, view.key.epoch));
        views
    }

    fn live_dependents(&self) -> Vec<StreamAttachmentKeyV1> {
        let mut dependents = self
            .attachments
            .values()
            .filter_map(|attachment| {
                (!matches!(
                    attachment.state,
                    IndexedStreamAttachmentState::Finalized { .. }
                ))
                .then_some(attachment.key.clone())
            })
            .collect::<Vec<_>>();
        dependents.sort_by_key(|key| (key.stream_id, key.attachment_id, key.epoch));
        dependents
    }

    fn incomplete_cascade_dependents(&self) -> Vec<StreamAttachmentKeyV1> {
        self.live_dependents()
            .into_iter()
            .filter(|key| !self.cascade_outbox.contains_key(key))
            .collect()
    }
}

fn validate_attachment_epoch(
    existing: &IndexedStreamAttachment,
    requested: &StreamAttachmentKeyV1,
) -> Result<(), DurableStreamProducerError> {
    if requested.epoch < existing.key.epoch {
        Err(DurableStreamProducerError::StaleEpoch {
            current: existing.key.epoch,
            actual: requested.epoch,
        })
    } else if requested.epoch > existing.key.epoch {
        Err(DurableStreamProducerError::InvalidEpoch {
            current: existing.key.epoch,
            actual: requested.epoch,
        })
    } else {
        Ok(())
    }
}

fn attachment_identity_matches_except_epoch(
    existing: &StreamAttachmentKeyV1,
    requested: &StreamAttachmentKeyV1,
) -> bool {
    existing.attachment_id == requested.attachment_id
        && existing.stream_id == requested.stream_id
        && existing.session_key == requested.session_key
        && existing.producer_environment_id == requested.producer_environment_id
        && existing.producer == requested.producer
        && existing.expected_producer_fingerprint == requested.expected_producer_fingerprint
        && existing.consumer_environment_id == requested.consumer_environment_id
        && existing.consumer == requested.consumer
        && existing.expected_consumer_fingerprint == requested.expected_consumer_fingerprint
        && existing.consumer_invocation == requested.consumer_invocation
}

fn validate_terminal_sequence(
    stream: &IndexedProducerStream,
    stream_id: StreamId,
    sequence: u64,
) -> Result<(), DurableStreamProducerError> {
    if stream.terminal {
        return Err(DurableStreamProducerError::AlreadyTerminal(stream_id));
    }
    if sequence != stream.next_sequence {
        return Err(DurableStreamProducerError::SequenceGap {
            expected: stream.next_sequence,
            actual: sequence,
        });
    }
    Ok(())
}

fn validate_version(version: u8) -> Result<(), DurableStreamProducerError> {
    if version == DURABLE_STREAM_FORMAT_VERSION {
        Ok(())
    } else {
        Err(DurableStreamProducerError::UnsupportedVersion(version))
    }
}

fn validate_items_payload(
    payload: &StreamItemsPayloadV1,
) -> Result<(), DurableStreamProducerError> {
    match payload {
        StreamItemsPayloadV1::Values(values) if values.len() != 1 => {
            crate::metrics::durable_stream::record_limit_violation("value_batch_items");
            Err(DurableStreamProducerError::InvalidValueBatch)
        }
        StreamItemsPayloadV1::Values(values)
            if values
                .first()
                .is_some_and(|value| value.len() > MAX_DURABLE_STREAM_ITEM_SIZE) =>
        {
            crate::metrics::durable_stream::record_limit_violation("item_size");
            Err(DurableStreamProducerError::ItemTooLarge)
        }
        StreamItemsPayloadV1::PackedU8(bytes)
            if bytes.is_empty() || bytes.len() > MAX_PACKED_U8_STREAM_ITEM_SIZE =>
        {
            crate::metrics::durable_stream::record_limit_violation("packed_u8_batch_size");
            Err(DurableStreamProducerError::InvalidPackedU8Batch)
        }
        _ => Ok(()),
    }
}

fn logical_payloads(payload: &StreamItemsPayloadV1) -> Vec<CommittedProducerStreamEventPayloadV1> {
    match payload {
        StreamItemsPayloadV1::Values(values) => values
            .iter()
            .cloned()
            .map(CommittedProducerStreamEventPayloadV1::Value)
            .collect(),
        StreamItemsPayloadV1::PackedU8(bytes) => bytes
            .iter()
            .copied()
            .map(CommittedProducerStreamEventPayloadV1::PackedU8)
            .collect(),
    }
}

fn registration_coordinate_depth(coordinate: &StreamRegistrationCoordinateV1) -> usize {
    match coordinate {
        StreamRegistrationCoordinateV1::Root {
            recursive_value_path,
            ..
        }
        | StreamRegistrationCoordinateV1::Nested {
            recursive_value_path,
            ..
        } => recursive_value_path.len(),
    }
}

fn nested_coordinate_matches_item(
    coordinate: &StreamRegistrationCoordinateV1,
    stream_id: StreamId,
    first_sequence: u64,
    logical_item_count: u64,
) -> bool {
    matches!(
        coordinate,
        StreamRegistrationCoordinateV1::Nested {
            parent_stream_id,
            parent_producer_sequence,
            ..
        } if *parent_stream_id == stream_id
            && parent_producer_sequence
                .checked_sub(first_sequence)
                .is_some_and(|relative_sequence| relative_sequence < logical_item_count)
    )
}

fn resource_exhausted_error_context() -> Result<Vec<u8>, DurableStreamProducerError> {
    let error = AgentError::CustomError(TypedSchemaValue::new(
        SchemaGraph::anonymous(SchemaType::string()),
        SchemaValue::String("ResourceExhausted".to_string()),
    ));
    golem_common::serialization::serialize(&error).map_err(DurableStreamProducerError::Oplog)
}

pub(crate) type DurableStreamCommit = Arc<
    dyn Fn(Option<oneshot::Sender<()>>) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync,
>;

pub(crate) struct DurableStreamProducer {
    oplog: Arc<dyn Oplog>,
    commit: DurableStreamCommit,
    environment_id: EnvironmentId,
    producer: AgentId,
    producer_fingerprint: AgentFingerprint,
    index: Mutex<ProducerStreamIndex>,
    buses: RwLock<HashMap<StreamId, Arc<DurableLiveStreamBus<CommittedProducerStreamEventV1>>>>,
    source_cancellations: RwLock<HashMap<StreamId, (u64, CancellationToken)>>,
    next_source_cancellation_id: AtomicU64,
    reconciliation_cursor: AtomicUsize,
    open_stream_count: AtomicUsize,
    live_join_capacity: usize,
    session_records_changed: Notify,
    session_locks: std::sync::Mutex<HashMap<StreamSessionKeyV1, Arc<tokio::sync::Mutex<()>>>>,
}

impl DurableStreamProducer {
    #[cfg(test)]
    pub(crate) async fn load(
        oplog: Arc<dyn Oplog>,
        environment_id: EnvironmentId,
        producer: AgentId,
        producer_fingerprint: AgentFingerprint,
        live_join_capacity: Option<usize>,
    ) -> Result<Arc<Self>, DurableStreamProducerError> {
        let commit_oplog = oplog.clone();
        let commit: DurableStreamCommit = Arc::new(move |committed| {
            let oplog = commit_oplog.clone();
            Box::pin(async move {
                oplog.commit(CommitLevel::Always).await;
                if let Some(committed) = committed {
                    let _ = committed.send(());
                }
            })
        });
        Self::load_with_commit(
            oplog,
            environment_id,
            producer,
            producer_fingerprint,
            live_join_capacity,
            commit,
        )
        .await
    }

    pub(crate) async fn load_with_commit(
        oplog: Arc<dyn Oplog>,
        environment_id: EnvironmentId,
        producer: AgentId,
        producer_fingerprint: AgentFingerprint,
        live_join_capacity: Option<usize>,
        commit: DurableStreamCommit,
    ) -> Result<Arc<Self>, DurableStreamProducerError> {
        let live_join_capacity = live_join_capacity.unwrap_or(DEFAULT_LIVE_JOIN_BUFFER_SIZE);
        DurableLiveStreamBus::<CommittedProducerStreamEventV1>::new(live_join_capacity)?;
        let mut index = ProducerStreamIndex::default();
        let mut pending_nested_registrations = Vec::new();
        let current_index = oplog.current_oplog_index().await;
        if current_index.is_defined() {
            let entries = oplog
                .read_exact(OplogIndex::INITIAL, current_index.as_u64())
                .await;
            for (oplog_index, entry) in entries {
                match entry {
                    OplogEntry::StreamRegistered { record, .. } => {
                        let record = oplog
                            .download_payload(record)
                            .await
                            .map_err(DurableStreamProducerError::Oplog)?;
                        if matches!(
                            &record.coordinate,
                            StreamRegistrationCoordinateV1::Nested { .. }
                        ) {
                            pending_nested_registrations.push((oplog_index, record));
                        } else {
                            if !pending_nested_registrations.is_empty() {
                                return Err(DurableStreamProducerError::CorruptHistory(
                                    "nested registration batch is missing its enclosing item"
                                        .to_string(),
                                ));
                            }
                            index.apply_registration(
                                oplog_index,
                                record,
                                environment_id,
                                &producer,
                                producer_fingerprint,
                            )?;
                        }
                    }
                    OplogEntry::StreamItems { record, .. } => {
                        let record = oplog
                            .download_payload(record)
                            .await
                            .map_err(DurableStreamProducerError::Oplog)?;
                        index.apply_item_batch(
                            oplog_index,
                            std::mem::take(&mut pending_nested_registrations),
                            record,
                            environment_id,
                            &producer,
                            producer_fingerprint,
                        )?;
                    }
                    OplogEntry::StreamEnd { record, .. } => {
                        if !pending_nested_registrations.is_empty() {
                            return Err(DurableStreamProducerError::CorruptHistory(
                                "nested registration batch is missing its enclosing item"
                                    .to_string(),
                            ));
                        }
                        let record = oplog
                            .download_payload(record)
                            .await
                            .map_err(DurableStreamProducerError::Oplog)?;
                        index.apply_end(oplog_index, record, producer_fingerprint)?;
                    }
                    OplogEntry::StreamCancel { record, .. } => {
                        if !pending_nested_registrations.is_empty() {
                            return Err(DurableStreamProducerError::CorruptHistory(
                                "nested registration batch is missing its enclosing item"
                                    .to_string(),
                            ));
                        }
                        let record = oplog
                            .download_payload(record)
                            .await
                            .map_err(DurableStreamProducerError::Oplog)?;
                        index.apply_cancel(oplog_index, record, producer_fingerprint)?;
                    }
                    OplogEntry::StreamSession { record, .. } => {
                        if !pending_nested_registrations.is_empty() {
                            return Err(DurableStreamProducerError::CorruptHistory(
                                "nested registration batch is missing its enclosing item"
                                    .to_string(),
                            ));
                        }
                        let record = oplog
                            .download_payload(record)
                            .await
                            .map_err(DurableStreamProducerError::Oplog)?;
                        index.apply_session_references(&record)?;
                        index.apply_deletion_record(
                            &record,
                            environment_id,
                            &producer,
                            producer_fingerprint,
                        )?;
                        index.apply_attachment_record(
                            &record,
                            environment_id,
                            &producer,
                            producer_fingerprint,
                        )?;
                        if let StreamSessionRecordV1::Finished(record) = &record {
                            index.apply_finished(record)?;
                        }
                    }
                    _ => {
                        if !pending_nested_registrations.is_empty() {
                            return Err(DurableStreamProducerError::CorruptHistory(
                                "nested registration batch is missing its enclosing item"
                                    .to_string(),
                            ));
                        }
                    }
                }
            }
        }
        if !pending_nested_registrations.is_empty() {
            return Err(DurableStreamProducerError::CorruptHistory(
                "nested registration batch is missing its enclosing item".to_string(),
            ));
        }

        let mut buses = HashMap::with_capacity(index.streams.len());
        for (stream_id, stream) in &index.streams {
            let bus = Arc::new(DurableLiveStreamBus::new(live_join_capacity)?);
            for event in stream.events.values() {
                bus.publish_committed(DurableLiveStreamEvent {
                    offset: event.offset,
                    payload: event.clone(),
                })
                .await?;
            }
            buses.insert(*stream_id, bus);
        }

        let open_stream_count = index
            .streams
            .values()
            .filter(|stream| !stream.terminal)
            .count();
        crate::metrics::durable_stream::add_open_streams(open_stream_count);
        if !index.streams.is_empty() {
            tracing::debug!(
                recovered_streams = index.streams.len(),
                recovered_open_streams = open_stream_count,
                "Durable stream producer index recovered"
            );
        }
        Ok(Arc::new(Self {
            oplog,
            commit,
            environment_id,
            producer,
            producer_fingerprint,
            index: Mutex::new(index),
            buses: RwLock::new(buses),
            source_cancellations: RwLock::new(HashMap::new()),
            next_source_cancellation_id: AtomicU64::new(1),
            reconciliation_cursor: AtomicUsize::new(0),
            open_stream_count: AtomicUsize::new(open_stream_count),
            live_join_capacity,
            session_records_changed: Notify::new(),
            session_locks: std::sync::Mutex::new(HashMap::new()),
        }))
    }

    async fn commit(&self) {
        (self.commit)(None).await;
    }

    async fn commit_notifying(&self, committed: oneshot::Sender<()>) {
        (self.commit)(Some(committed)).await;
    }

    fn record_registered_streams(&self, count: usize) {
        self.open_stream_count.fetch_add(count, Ordering::Relaxed);
        crate::metrics::durable_stream::add_open_streams(count);
        for _ in 0..count {
            crate::metrics::durable_stream::record_lifecycle("registered");
        }
    }

    fn record_terminal_streams(&self, count: usize) {
        self.open_stream_count.fetch_sub(count, Ordering::Relaxed);
        crate::metrics::durable_stream::remove_open_streams(count);
        for _ in 0..count {
            crate::metrics::durable_stream::record_lifecycle("terminal");
        }
    }

    pub(crate) async fn append_session_record(
        &self,
        record: StreamSessionRecordV1,
    ) -> Result<(), DurableStreamProducerError> {
        if !record.has_supported_format() {
            return Err(DurableStreamProducerError::CorruptHistory(
                "unsupported or malformed durable Stream Session record".to_string(),
            ));
        }
        let mut index = self.index.lock().await;
        if index.deleting
            && !matches!(
                &record,
                StreamSessionRecordV1::AttachmentFinalized(_)
                    | StreamSessionRecordV1::CascadeOutbox(_)
                    | StreamSessionRecordV1::ConsumerDeleting(_)
                    | StreamSessionRecordV1::SourceUnavailable(_)
            )
        {
            return Err(DurableStreamProducerError::ProducerDeleting);
        }
        if index.consumer_deleting
            && matches!(
                &record,
                StreamSessionRecordV1::TopologyPrepared(_)
                    | StreamSessionRecordV1::TopologyActivated(_)
            )
        {
            return Err(DurableStreamProducerError::ConsumerDeleting);
        }
        index.apply_session_references(&record)?;
        index.apply_deletion_record(
            &record,
            self.environment_id,
            &self.producer,
            self.producer_fingerprint,
        )?;
        self.oplog
            .add(OplogEntry::stream_session(OplogPayload::Inline(Box::new(
                record,
            ))))
            .await;
        self.commit().await;
        drop(index);
        self.notify_session_records_changed();
        Ok(())
    }

    pub(crate) async fn commit_source_unavailable_overlay(
        &self,
        key: StreamAttachmentKeyV1,
        source_offset: StreamOffsetV1,
        consumer_read_ordinal: u64,
    ) -> Result<bool, DurableStreamProducerError> {
        if key.consumer_environment_id != self.environment_id
            || key.consumer != self.producer
            || key.expected_consumer_fingerprint != self.producer_fingerprint
        {
            return Err(DurableStreamProducerError::InvalidAttachmentState);
        }
        let mut index = self.index.lock().await;
        let current = self.oplog.current_oplog_index().await;
        let mut source_offsets = Vec::new();
        let mut overlay = None;
        if current.is_defined() {
            for (_, entry) in self
                .oplog
                .read_exact(OplogIndex::INITIAL, current.as_u64())
                .await
            {
                let OplogEntry::StreamSession { record, .. } = entry else {
                    continue;
                };
                let record = self
                    .oplog
                    .download_payload(record)
                    .await
                    .map_err(DurableStreamProducerError::Oplog)?;
                match record {
                    StreamSessionRecordV1::ConsumerItemValue(record)
                        if record.session_key == key.session_key
                            && record.stream_id == key.stream_id =>
                    {
                        if record.consumer_read_ordinal != source_offsets.len() as u64 {
                            return Err(DurableStreamProducerError::CorruptHistory(
                                "consumer value journal contains a read-ordinal gap".to_string(),
                            ));
                        }
                        for index in 0..record.logical_item_count() {
                            source_offsets.push(record.source_offset_at(index).ok_or_else(
                                || {
                                    DurableStreamProducerError::CorruptHistory(
                                        "packed-u8 consumer journal offset range is invalid"
                                            .to_string(),
                                    )
                                },
                            )?);
                        }
                    }
                    StreamSessionRecordV1::ConsumerTerminal(record)
                        if record.session_key == key.session_key
                            && record.stream_id == key.stream_id =>
                    {
                        if record.consumer_read_ordinal != source_offsets.len() as u64 {
                            return Err(DurableStreamProducerError::CorruptHistory(
                                "consumer terminal journal contains a read-ordinal gap".to_string(),
                            ));
                        }
                        source_offsets.push(record.source_offset);
                    }
                    StreamSessionRecordV1::SourceUnavailable(record)
                        if record.key.session_key == key.session_key
                            && record.key.stream_id == key.stream_id =>
                    {
                        if record.consumer_read_ordinal != source_offsets.len() as u64 {
                            return Err(DurableStreamProducerError::CorruptHistory(
                                "source-unavailable overlay contains a read-ordinal gap"
                                    .to_string(),
                            ));
                        }
                        match &overlay {
                            Some(existing) if existing != &record => {
                                return Err(DurableStreamProducerError::CorruptHistory(
                                    "conflicting source-unavailable overlays".to_string(),
                                ));
                            }
                            Some(_) => {}
                            None => overlay = Some(record),
                        }
                    }
                    _ => {}
                }
            }
        }
        if let Some(existing) = overlay {
            return if existing.key == key
                && existing.source_offset == source_offset
                && existing.consumer_read_ordinal == consumer_read_ordinal
            {
                Ok(true)
            } else {
                Err(DurableStreamProducerError::AttachmentConflict)
            };
        }
        if source_offsets.len() as u64 != consumer_read_ordinal {
            return Err(DurableStreamProducerError::ConsumerJournalAdvanced);
        }
        let record = StreamSessionRecordV1::SourceUnavailable(StreamSourceUnavailableRecordV1 {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            key,
            source_offset,
            consumer_read_ordinal,
        });
        index.apply_consumer_journal_record(&record)?;
        self.oplog
            .add(OplogEntry::stream_session(OplogPayload::Inline(Box::new(
                record,
            ))))
            .await;
        self.commit().await;
        self.notify_session_records_changed();
        Ok(false)
    }

    async fn persist_attachment_record(
        &self,
        record: StreamSessionRecordV1,
    ) -> Result<AttachmentApplyOutcome, DurableStreamProducerError> {
        if !record.has_supported_format() {
            return Err(DurableStreamProducerError::CorruptHistory(
                "unsupported or malformed durable attachment record".to_string(),
            ));
        }
        let mut index = self.index.lock().await;
        let mut updated = index.clone();
        let outcome = updated.apply_attachment_record(
            &record,
            self.environment_id,
            &self.producer,
            self.producer_fingerprint,
        )?;
        if outcome == AttachmentApplyOutcome::Changed {
            self.oplog
                .add(OplogEntry::stream_session(OplogPayload::Inline(Box::new(
                    record,
                ))))
                .await;
            self.commit().await;
            *index = updated;
        }
        drop(index);
        if outcome == AttachmentApplyOutcome::Changed {
            self.notify_session_records_changed();
        }
        Ok(outcome)
    }

    pub(crate) fn session_lock(
        &self,
        session_key: &StreamSessionKeyV1,
    ) -> Arc<tokio::sync::Mutex<()>> {
        self.session_locks
            .lock()
            .expect("durable stream session lock map poisoned")
            .entry(session_key.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    pub(crate) fn environment_id(&self) -> EnvironmentId {
        self.environment_id
    }

    pub(crate) fn agent_id(&self) -> &AgentId {
        &self.producer
    }

    pub(crate) fn fingerprint(&self) -> AgentFingerprint {
        self.producer_fingerprint
    }

    pub(crate) async fn ensure_session_accepts_new_events(
        &self,
        session_key: &StreamSessionKeyV1,
    ) -> Result<(), DurableStreamProducerError> {
        if self
            .index
            .lock()
            .await
            .finished_sessions
            .contains(session_key)
        {
            Err(DurableStreamProducerError::SessionFinished(
                session_key.clone(),
            ))
        } else {
            Ok(())
        }
    }

    pub(crate) fn session_records_changed(&self) -> &Notify {
        &self.session_records_changed
    }

    pub(crate) fn notify_session_records_changed(&self) {
        self.session_records_changed.notify_waiters();
    }

    #[tracing::instrument(name = "durable_stream.register", skip_all)]
    pub(crate) async fn register(
        &self,
        request: ProducerRegistrationRequestV1,
    ) -> Result<ProducerWriteOutcomeV1<DurableStreamHandleV1>, DurableStreamProducerError> {
        if registration_coordinate_depth(&request.coordinate) > MAX_STREAM_VALUE_TRAVERSAL_DEPTH {
            crate::metrics::durable_stream::record_limit_violation("traversal_depth");
            return Err(DurableStreamProducerError::TraversalDepthLimit);
        }
        let mut index = self.index.lock().await;
        if let Some(stream_id) = index.coordinates.get(&request.coordinate) {
            let existing = index
                .registrations
                .get(stream_id)
                .expect("coordinate index points at a missing registration");
            if registration_matches(existing, &request) {
                crate::metrics::durable_stream::record_producer_operation("register", true);
                tracing::debug!(
                    stream_id = %existing.handle.stream_id,
                    registration_oplog_index = existing.registration_oplog_index.as_u64(),
                    replayed = true,
                    "Durable stream registration resolved"
                );
                return Ok(ProducerWriteOutcomeV1 {
                    value: existing.handle.clone(),
                    replayed: true,
                });
            }
            return Err(DurableStreamProducerError::RegistrationDivergence);
        }
        index.ensure_producer_write_allowed()?;
        if !matches!(
            &request.coordinate,
            StreamRegistrationCoordinateV1::Root { .. }
        ) {
            return Err(DurableStreamProducerError::RegistrationDivergence);
        }
        let session_key = index
            .registration_session_key(&request.coordinate, &request.session_mapping)
            .ok_or_else(|| match &request.coordinate {
                StreamRegistrationCoordinateV1::Nested {
                    parent_stream_id, ..
                } => DurableStreamProducerError::UnknownStream(*parent_stream_id),
                StreamRegistrationCoordinateV1::Root { .. } => {
                    unreachable!("root registration always defines its session")
                }
            })?;
        if index.finished_sessions.contains(&session_key) {
            return Err(DurableStreamProducerError::SessionFinished(session_key));
        }
        if index
            .session_stream_counts
            .get(&session_key)
            .copied()
            .unwrap_or_default()
            >= MAX_DURABLE_STREAMS_PER_SESSION
        {
            crate::metrics::durable_stream::record_limit_violation("streams_per_session");
            return Err(DurableStreamProducerError::StreamLimit);
        }
        StreamId::derive(
            self.environment_id,
            &self.producer,
            self.producer_fingerprint,
            OplogIndex::INITIAL,
        )
        .map_err(|error| DurableStreamProducerError::CorruptHistory(error.to_string()))?;

        let environment_id = self.environment_id;
        let producer = self.producer.clone();
        let producer_fingerprint = self.producer_fingerprint;
        let request_for_entry = request.clone();
        let mut entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |oplog_index| {
                vec![DurableStreamOplogRecord::Registered(registration_record(
                    oplog_index,
                    environment_id,
                    producer,
                    producer_fingerprint,
                    request_for_entry,
                ))]
            }))
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        self.commit().await;
        let (oplog_index, entry) = entries
            .pop()
            .expect("registration batch returned no oplog entry");
        let OplogEntry::StreamRegistered { record, .. } = entry else {
            unreachable!("registration builder returned a different oplog entry")
        };
        let record = self
            .oplog
            .download_payload(record)
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        index.apply_registration(
            oplog_index,
            record.clone(),
            self.environment_id,
            &self.producer,
            self.producer_fingerprint,
        )?;
        self.buses
            .write()
            .expect("durable stream bus map lock poisoned")
            .insert(
                record.handle.stream_id,
                Arc::new(DurableLiveStreamBus::new(self.live_join_capacity)?),
            );
        self.record_registered_streams(1);
        crate::metrics::durable_stream::record_producer_operation("register", false);
        tracing::debug!(
            stream_id = %record.handle.stream_id,
            registration_oplog_index = record.registration_oplog_index.as_u64(),
            replayed = false,
            "Durable stream registration committed"
        );
        Ok(ProducerWriteOutcomeV1 {
            value: record.handle,
            replayed: false,
        })
    }

    /// Atomically registers root input streams, prepares the session, and attaches its invocation.
    pub(crate) async fn prepare_session(
        &self,
        requests: Vec<(u64, ProducerRegistrationRequestV1)>,
        pending_invocation: OplogEntry,
        committed: oneshot::Sender<()>,
        make_prepared: impl FnOnce(Vec<(u64, DurableStreamHandleV1)>) -> StreamSessionPreparedRecordV1
        + Send
        + 'static,
    ) -> Result<StreamSessionPreparedRecordV1, DurableStreamProducerError> {
        let mut index = self.index.lock().await;
        index.ensure_producer_write_allowed()?;
        if requests.len() > MAX_DURABLE_STREAMS_PER_SESSION {
            crate::metrics::durable_stream::record_limit_violation("streams_per_session");
            return Err(DurableStreamProducerError::StreamLimit);
        }
        for (_, request) in &requests {
            if registration_coordinate_depth(&request.coordinate) > MAX_STREAM_VALUE_TRAVERSAL_DEPTH
            {
                crate::metrics::durable_stream::record_limit_violation("traversal_depth");
                return Err(DurableStreamProducerError::TraversalDepthLimit);
            }
            if !matches!(
                request.coordinate,
                StreamRegistrationCoordinateV1::Root { .. }
            ) || index.coordinates.contains_key(&request.coordinate)
            {
                return Err(DurableStreamProducerError::RegistrationDivergence);
            }
            if let Some(session_key) =
                index.registration_session_key(&request.coordinate, &request.session_mapping)
                && index.finished_sessions.contains(&session_key)
            {
                return Err(DurableStreamProducerError::SessionFinished(session_key));
            }
        }

        let environment_id = self.environment_id;
        let producer = self.producer.clone();
        let producer_fingerprint = self.producer_fingerprint;
        let records = requests.clone();
        let entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |first_index| {
                let mut result = Vec::with_capacity(records.len() + 3);
                let mut handles = Vec::with_capacity(records.len());
                for (sub_index, (transport_stream_id, request)) in records.into_iter().enumerate() {
                    let oplog_index = OplogIndex::from_u64(
                        first_index.as_u64()
                            + u64::try_from(sub_index)
                                .expect("durable stream batch size fits in u64"),
                    );
                    let record = registration_record(
                        oplog_index,
                        environment_id,
                        producer.clone(),
                        producer_fingerprint,
                        request,
                    );
                    handles.push((transport_stream_id, record.handle.clone()));
                    result.push(DurableStreamOplogRecord::Registered(record));
                }
                let prepared = make_prepared(handles);
                let prepared_record = StreamSessionRecordV1::Prepared(prepared);
                if !prepared_record.has_supported_format() {
                    return Vec::new();
                }
                let pending_invocation_oplog_index = OplogIndex::from_u64(
                    first_index.as_u64()
                        + u64::try_from(result.len() + 1)
                            .expect("durable stream batch size fits in u64"),
                );
                let StreamSessionRecordV1::Prepared(prepared) = &prepared_record else {
                    unreachable!()
                };
                let attached = StreamSessionAttachedRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: prepared.attempt.session_key.clone(),
                    attachment_id: prepared.attempt.attachment_id,
                    attempt_id: prepared.attempt.attempt_id,
                    epoch: 1,
                    pending_invocation_oplog_index,
                };
                result.push(DurableStreamOplogRecord::Session(Box::new(prepared_record)));
                result.push(DurableStreamOplogRecord::InlineEntry(pending_invocation));
                result.push(DurableStreamOplogRecord::Session(Box::new(
                    StreamSessionRecordV1::Attached(attached),
                )));
                result
            }))
            .await
            .map_err(DurableStreamProducerError::Oplog)?;

        let mut prepared = None;
        let mut registrations = Vec::with_capacity(requests.len());
        for (oplog_index, entry) in entries {
            match entry {
                OplogEntry::StreamRegistered { record, .. } => {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(DurableStreamProducerError::Oplog)?;
                    registrations.push((oplog_index, record));
                }
                OplogEntry::StreamSession { record, .. } => {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(DurableStreamProducerError::Oplog)?;
                    match record {
                        StreamSessionRecordV1::Prepared(record) => prepared = Some(record),
                        StreamSessionRecordV1::Attached(_) => {}
                        _ => {
                            return Err(DurableStreamProducerError::CorruptHistory(
                                "preparation batch contains an unexpected session record"
                                    .to_string(),
                            ));
                        }
                    }
                }
                OplogEntry::PendingAgentInvocation { .. } => {}
                _ => {
                    return Err(DurableStreamProducerError::CorruptHistory(
                        "preparation batch contains an unexpected oplog entry".to_string(),
                    ));
                }
            }
        }
        let prepared = prepared.ok_or_else(|| {
            DurableStreamProducerError::CorruptHistory(
                "preparation batch contains no Prepared session record".to_string(),
            )
        })?;
        let mut updated_index = index.clone();
        let mut buses = Vec::with_capacity(registrations.len());
        for (oplog_index, record) in registrations {
            updated_index.apply_registration(
                oplog_index,
                record.clone(),
                self.environment_id,
                &self.producer,
                self.producer_fingerprint,
            )?;
            buses.push((
                record.handle.stream_id,
                Arc::new(DurableLiveStreamBus::new(self.live_join_capacity)?),
            ));
        }

        self.commit_notifying(committed).await;
        *index = updated_index;
        self.buses
            .write()
            .expect("durable stream bus map lock poisoned")
            .extend(buses);
        self.record_registered_streams(requests.len());
        crate::metrics::durable_stream::record_producer_operation("prepare_session", false);
        tracing::debug!(
            attachment_id = %prepared.attempt.attachment_id.0,
            attempt_id = %prepared.attempt.attempt_id.0,
            epoch = 1_u64,
            registered_streams = requests.len(),
            "Durable Stream Session preparation committed"
        );
        Ok(prepared)
    }

    pub(crate) async fn register_result_streams(
        &self,
        requests: Vec<ProducerRegistrationRequestV1>,
        make_result: impl FnOnce(Vec<DurableStreamHandleV1>) -> StreamSessionRecordV1 + Send + 'static,
    ) -> Result<(Vec<DurableStreamHandleV1>, StreamSessionRecordV1), DurableStreamProducerError>
    {
        let mut index = self.index.lock().await;
        index.ensure_producer_write_allowed()?;
        if requests.len() > MAX_NEW_STREAM_HANDLES_PER_VALUE {
            crate::metrics::durable_stream::record_limit_violation("streams_per_value");
            return Err(DurableStreamProducerError::ValueStreamLimit);
        }
        if requests.is_empty() {
            let expected = make_result(Vec::new());
            let StreamSessionRecordV1::InvocationResult(expected_result) = &expected else {
                return Err(DurableStreamProducerError::CorruptHistory(
                    "empty result registration did not build an invocation-result record"
                        .to_string(),
                ));
            };
            if !expected.has_supported_format() {
                return Err(DurableStreamProducerError::CorruptHistory(
                    "result registration built a malformed invocation-result record".to_string(),
                ));
            }
            let session_finished = index
                .finished_sessions
                .contains(&expected_result.session_key);
            let session_key = expected_result.session_key.clone();
            drop(index);
            let current = self.oplog.current_oplog_index().await;
            if current.is_defined() {
                for (_, entry) in self
                    .oplog
                    .read_exact(OplogIndex::INITIAL, current.as_u64())
                    .await
                {
                    let OplogEntry::StreamSession { record, .. } = entry else {
                        continue;
                    };
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(DurableStreamProducerError::Oplog)?;
                    if let StreamSessionRecordV1::InvocationResult(result) = &record
                        && result.session_key == expected_result.session_key
                    {
                        return if record == expected {
                            Ok((Vec::new(), record))
                        } else {
                            Err(DurableStreamProducerError::RegistrationDivergence)
                        };
                    }
                }
            }
            if session_finished {
                return Err(DurableStreamProducerError::SessionFinished(session_key));
            }
            let expected_for_entry = expected.clone();
            let mut entries = self
                .oplog
                .add_durable_stream_batch(Box::new(move |_| {
                    vec![DurableStreamOplogRecord::Session(Box::new(
                        expected_for_entry,
                    ))]
                }))
                .await
                .map_err(DurableStreamProducerError::Oplog)?;
            self.commit().await;
            let (_, entry) = entries.pop().ok_or_else(|| {
                DurableStreamProducerError::CorruptHistory(
                    "empty result registration batch returned no session record".to_string(),
                )
            })?;
            let OplogEntry::StreamSession { record, .. } = entry else {
                return Err(DurableStreamProducerError::CorruptHistory(
                    "empty result registration batch returned an unexpected entry".to_string(),
                ));
            };
            let record = self
                .oplog
                .download_payload(record)
                .await
                .map_err(DurableStreamProducerError::Oplog)?;
            self.index.lock().await.apply_session_references(&record)?;
            return Ok((Vec::new(), record));
        }
        let existing_handles = requests
            .iter()
            .map(|request| {
                index
                    .coordinates
                    .get(&request.coordinate)
                    .and_then(|stream_id| index.registrations.get(stream_id))
                    .filter(|registration| registration_matches(registration, request))
                    .map(|registration| registration.handle.clone())
            })
            .collect::<Vec<_>>();
        if existing_handles.iter().any(Option::is_some) {
            if existing_handles.iter().any(Option::is_none) {
                return Err(DurableStreamProducerError::RegistrationDivergence);
            }
            let handles = existing_handles
                .into_iter()
                .map(Option::unwrap)
                .collect::<Vec<_>>();
            let expected = make_result(handles.clone());
            drop(index);
            let current = self.oplog.current_oplog_index().await;
            if current.is_defined() {
                for (_, entry) in self
                    .oplog
                    .read_exact(OplogIndex::INITIAL, current.as_u64())
                    .await
                {
                    let OplogEntry::StreamSession { record, .. } = entry else {
                        continue;
                    };
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(DurableStreamProducerError::Oplog)?;
                    if record == expected {
                        crate::metrics::durable_stream::record_producer_operation(
                            "register_result",
                            true,
                        );
                        return Ok((handles, record));
                    }
                }
            }
            return Err(DurableStreamProducerError::RegistrationDivergence);
        }
        let session_key = requests.first().and_then(|request| {
            index.registration_session_key(&request.coordinate, &request.session_mapping)
        });
        if let Some(session_key) = session_key {
            if index.finished_sessions.contains(&session_key) {
                return Err(DurableStreamProducerError::SessionFinished(session_key));
            }
            let current = index
                .session_stream_counts
                .get(&session_key)
                .copied()
                .unwrap_or_default();
            if current
                .checked_add(requests.len())
                .is_none_or(|count| count > MAX_DURABLE_STREAMS_PER_SESSION)
            {
                crate::metrics::durable_stream::record_limit_violation("streams_per_session");
                return Err(DurableStreamProducerError::StreamLimit);
            }
        }
        for request in &requests {
            if registration_coordinate_depth(&request.coordinate) > MAX_STREAM_VALUE_TRAVERSAL_DEPTH
            {
                crate::metrics::durable_stream::record_limit_violation("traversal_depth");
                return Err(DurableStreamProducerError::TraversalDepthLimit);
            }
            if !matches!(
                request.coordinate,
                StreamRegistrationCoordinateV1::Root { .. }
            ) || index.coordinates.contains_key(&request.coordinate)
            {
                return Err(DurableStreamProducerError::RegistrationDivergence);
            }
        }

        let environment_id = self.environment_id;
        let producer = self.producer.clone();
        let producer_fingerprint = self.producer_fingerprint;
        let entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |first_index| {
                let mut result = Vec::with_capacity(requests.len() + 1);
                let mut handles = Vec::with_capacity(requests.len());
                for (sub_index, request) in requests.into_iter().enumerate() {
                    let oplog_index = OplogIndex::from_u64(
                        first_index.as_u64()
                            + u64::try_from(sub_index)
                                .expect("durable stream batch size fits in u64"),
                    );
                    let record = registration_record(
                        oplog_index,
                        environment_id,
                        producer.clone(),
                        producer_fingerprint,
                        request,
                    );
                    handles.push(record.handle.clone());
                    result.push(DurableStreamOplogRecord::Registered(record));
                }
                let session_record = make_result(handles);
                if !session_record.has_supported_format() {
                    return Vec::new();
                }
                result.push(DurableStreamOplogRecord::Session(Box::new(session_record)));
                result
            }))
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        self.commit().await;

        let mut handles = Vec::new();
        let mut session_record = None;
        for (oplog_index, entry) in entries {
            match entry {
                OplogEntry::StreamRegistered { record, .. } => {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(DurableStreamProducerError::Oplog)?;
                    handles.push(record.handle.clone());
                    index.apply_registration(
                        oplog_index,
                        record.clone(),
                        self.environment_id,
                        &self.producer,
                        self.producer_fingerprint,
                    )?;
                    self.buses
                        .write()
                        .expect("durable stream bus map lock poisoned")
                        .insert(
                            record.handle.stream_id,
                            Arc::new(DurableLiveStreamBus::new(self.live_join_capacity)?),
                        );
                }
                OplogEntry::StreamSession { record, .. } => {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(DurableStreamProducerError::Oplog)?;
                    index.apply_session_references(&record)?;
                    session_record = Some(record);
                }
                _ => {
                    return Err(DurableStreamProducerError::CorruptHistory(
                        "result registration batch contains an unexpected oplog entry".to_string(),
                    ));
                }
            }
        }
        self.record_registered_streams(handles.len());
        crate::metrics::durable_stream::record_producer_operation("register_result", false);
        Ok((
            handles,
            session_record.ok_or_else(|| {
                DurableStreamProducerError::CorruptHistory(
                    "result registration batch contains no session record".to_string(),
                )
            })?,
        ))
    }

    #[cfg(test)]
    pub(crate) async fn write_items(
        &self,
        stream_id: StreamId,
        first_sequence: u64,
        payload: StreamItemsPayloadV1,
    ) -> Result<ProducerWriteOutcomeV1<Vec<StreamOffsetV1>>, DurableStreamProducerError> {
        self.write_items_with_nested(stream_id, first_sequence, payload, Vec::new())
            .await
    }

    pub(crate) async fn handle_for_coordinate(
        &self,
        coordinate: &StreamRegistrationCoordinateV1,
    ) -> Option<DurableStreamHandleV1> {
        let index = self.index.lock().await;
        index
            .coordinates
            .get(coordinate)
            .and_then(|stream_id| index.registrations.get(stream_id))
            .map(|registration| registration.handle.clone())
    }

    pub(crate) async fn validate_registration(
        &self,
        request: &ProducerRegistrationRequestV1,
    ) -> Result<DurableStreamHandleV1, DurableStreamProducerError> {
        let index = self.index.lock().await;
        let stream_id = index
            .coordinates
            .get(&request.coordinate)
            .ok_or(DurableStreamProducerError::RegistrationDivergence)?;
        let registration = index.registrations.get(stream_id).ok_or_else(|| {
            DurableStreamProducerError::CorruptHistory(
                "registration coordinate points at a missing registration".to_string(),
            )
        })?;
        if !registration_matches(registration, request) {
            return Err(DurableStreamProducerError::RegistrationDivergence);
        }
        Ok(registration.handle.clone())
    }

    pub(crate) async fn validate_new_session_stream_count(
        &self,
        session_key: &StreamSessionKeyV1,
        new_stream_count: usize,
    ) -> Result<(), DurableStreamProducerError> {
        if new_stream_count > MAX_NEW_STREAM_HANDLES_PER_VALUE {
            return Err(DurableStreamProducerError::ValueStreamLimit);
        }
        let index = self.index.lock().await;
        if new_stream_count != 0 && index.finished_sessions.contains(session_key) {
            return Err(DurableStreamProducerError::SessionFinished(
                session_key.clone(),
            ));
        }
        let current = index
            .session_stream_counts
            .get(session_key)
            .copied()
            .unwrap_or_default();
        if current
            .checked_add(new_stream_count)
            .is_none_or(|count| count > MAX_DURABLE_STREAMS_PER_SESSION)
        {
            return Err(DurableStreamProducerError::StreamLimit);
        }
        Ok(())
    }

    pub(crate) async fn nested_handles(
        &self,
        stream_id: StreamId,
        first_sequence: u64,
    ) -> Result<Vec<DurableStreamHandleV1>, DurableStreamProducerError> {
        let index = self.index.lock().await;
        let stream = index
            .streams
            .get(&stream_id)
            .ok_or(DurableStreamProducerError::UnknownStream(stream_id))?;
        let (_, _, nested_stream_ids) = stream
            .batches
            .get(&first_sequence)
            .ok_or(DurableStreamProducerError::EventConflict)?;
        nested_stream_ids
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
                    .ok_or(DurableStreamProducerError::UnknownStream(*stream_id))
            })
            .collect()
    }

    pub(crate) async fn input_high_water(
        &self,
        stream_id: StreamId,
    ) -> Result<Option<InputStreamHighWaterV1>, DurableStreamProducerError> {
        let index = self.index.lock().await;
        let stream = index
            .streams
            .get(&stream_id)
            .ok_or(DurableStreamProducerError::UnknownStream(stream_id))?;
        let Some((highest_contiguous_sequence, event)) = stream.events.last_key_value() else {
            return Ok(None);
        };
        Ok(Some(InputStreamHighWaterV1 {
            highest_contiguous_sequence: *highest_contiguous_sequence,
            resulting_offset: event.offset,
            terminal: stream.terminal,
        }))
    }

    pub(crate) async fn write_items_with_nested(
        &self,
        stream_id: StreamId,
        first_sequence: u64,
        payload: StreamItemsPayloadV1,
        nested: Vec<ProducerRegistrationRequestV1>,
    ) -> Result<ProducerWriteOutcomeV1<Vec<StreamOffsetV1>>, DurableStreamProducerError> {
        self.write_items_with_nested_sources_at_depth(
            stream_id,
            first_sequence,
            payload,
            nested
                .into_iter()
                .map(NestedStreamWriteV1::Register)
                .collect(),
            0,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn write_items_with_nested_at_depth(
        &self,
        stream_id: StreamId,
        first_sequence: u64,
        payload: StreamItemsPayloadV1,
        nested: Vec<ProducerRegistrationRequestV1>,
        traversal_depth: usize,
    ) -> Result<ProducerWriteOutcomeV1<Vec<StreamOffsetV1>>, DurableStreamProducerError> {
        self.write_items_with_nested_sources_at_depth(
            stream_id,
            first_sequence,
            payload,
            nested
                .into_iter()
                .map(NestedStreamWriteV1::Register)
                .collect(),
            traversal_depth,
        )
        .await
    }

    pub(crate) async fn write_items_with_nested_sources(
        &self,
        stream_id: StreamId,
        first_sequence: u64,
        payload: StreamItemsPayloadV1,
        nested: Vec<NestedStreamWriteV1>,
    ) -> Result<ProducerWriteOutcomeV1<Vec<StreamOffsetV1>>, DurableStreamProducerError> {
        self.write_items_with_nested_sources_at_depth(stream_id, first_sequence, payload, nested, 0)
            .await
    }

    #[tracing::instrument(
        name = "durable_stream.write",
        skip_all,
        fields(stream_id = %stream_id, first_sequence)
    )]
    async fn write_items_with_nested_sources_at_depth(
        &self,
        stream_id: StreamId,
        first_sequence: u64,
        payload: StreamItemsPayloadV1,
        nested_sources: Vec<NestedStreamWriteV1>,
        traversal_depth: usize,
    ) -> Result<ProducerWriteOutcomeV1<Vec<StreamOffsetV1>>, DurableStreamProducerError> {
        validate_items_payload(&payload)?;
        let logical_payloads = logical_payloads(&payload);
        let item_count = logical_payloads.len() as u64;
        let nested = nested_sources
            .iter()
            .filter_map(|source| match source {
                NestedStreamWriteV1::Register(request) => Some(request.clone()),
                NestedStreamWriteV1::Forward(_) => None,
            })
            .collect::<Vec<_>>();

        let mut index = self.index.lock().await;
        let session_key = index
            .stream_sessions
            .get(&stream_id)
            .cloned()
            .ok_or(DurableStreamProducerError::UnknownStream(stream_id))?;
        let session_finished = index.finished_sessions.contains(&session_key);
        let stream = index
            .streams
            .get(&stream_id)
            .ok_or(DurableStreamProducerError::UnknownStream(stream_id))?;
        if first_sequence < stream.next_sequence {
            if stream.batches.get(&first_sequence).is_some_and(
                |(committed_payload, _, nested_stream_ids)| {
                    committed_payload == &payload
                        && nested_stream_ids.len() == nested_sources.len()
                        && nested_stream_ids.iter().zip(&nested_sources).all(
                            |(stream_id, source)| match source {
                                NestedStreamWriteV1::Register(request) => index
                                    .registrations
                                    .get(stream_id)
                                    .is_some_and(|record| registration_matches(record, request)),
                                NestedStreamWriteV1::Forward(handle) => {
                                    stream_id == &handle.stream_id
                                }
                            },
                        )
                },
            ) {
                let offsets = stream
                    .batches
                    .get(&first_sequence)
                    .expect("validated replay batch is missing")
                    .1
                    .clone();
                let events = (first_sequence..first_sequence + item_count)
                    .map(|sequence| {
                        stream
                            .events
                            .get(&sequence)
                            .expect("validated replay batch event is missing")
                            .clone()
                    })
                    .collect();
                drop(index);
                self.publish_repair(stream_id, events).await?;
                crate::metrics::durable_stream::record_producer_operation("write", true);
                tracing::debug!(
                    stream_id = %stream_id,
                    first_sequence,
                    logical_item_count = item_count,
                    replayed = true,
                    "Durable stream item batch resolved"
                );
                return Ok(ProducerWriteOutcomeV1 {
                    value: offsets,
                    replayed: true,
                });
            }
            return Err(DurableStreamProducerError::EventConflict);
        }
        index.ensure_producer_write_allowed()?;
        if session_finished {
            return Err(DurableStreamProducerError::SessionFinished(session_key));
        }
        if stream.terminal {
            let terminal = stream
                .events
                .values()
                .next_back()
                .expect("terminal stream has no terminal event")
                .clone();
            let error = fenced_by_terminal(stream);
            drop(index);
            self.publish_repair(stream_id, vec![terminal]).await?;
            return Err(error);
        }
        if first_sequence != stream.next_sequence {
            return Err(DurableStreamProducerError::SequenceGap {
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
            self.commit_resource_exhausted_terminal(index, stream_id, first_sequence)
                .await?;
            crate::metrics::durable_stream::record_limit_violation("traversal_depth");
            return Err(DurableStreamProducerError::TraversalDepthLimit);
        }
        if nested_sources.len() > MAX_NEW_STREAM_HANDLES_PER_VALUE {
            self.commit_resource_exhausted_terminal(index, stream_id, first_sequence)
                .await?;
            crate::metrics::durable_stream::record_limit_violation("streams_per_value");
            return Err(DurableStreamProducerError::ValueStreamLimit);
        }
        if (matches!(payload, StreamItemsPayloadV1::PackedU8(_)) && !nested_sources.is_empty())
            || nested.iter().any(|request| {
                !nested_coordinate_matches_item(
                    &request.coordinate,
                    stream_id,
                    first_sequence,
                    item_count,
                )
            })
        {
            return Err(DurableStreamProducerError::RegistrationDivergence);
        }
        for source in &nested_sources {
            if let NestedStreamWriteV1::Forward(handle) = source
                && (handle.format_version != DURABLE_STREAM_FORMAT_VERSION
                    || index.referenced_handles.get(&handle.stream_id).is_none_or(
                        |(referenced_handle, referenced_sessions)| {
                            referenced_handle != handle
                                || !referenced_sessions.contains(&session_key)
                        },
                    ))
            {
                return Err(DurableStreamProducerError::InvalidHandle);
            }
        }
        let mut seen_coordinates = HashSet::with_capacity(nested.len());
        let mut existing_nested = HashMap::new();
        let mut new_nested = Vec::new();
        for request in &nested {
            if !seen_coordinates.insert(request.coordinate.clone()) {
                return Err(DurableStreamProducerError::RegistrationDivergence);
            }
            if let Some(existing_id) = index.coordinates.get(&request.coordinate) {
                let existing = index
                    .registrations
                    .get(existing_id)
                    .expect("coordinate index points at a missing registration");
                if !registration_matches(existing, request) {
                    return Err(DurableStreamProducerError::RegistrationDivergence);
                }
                existing_nested.insert(request.coordinate.clone(), *existing_id);
            } else {
                new_nested.push(request.clone());
            }
        }
        if new_nested.len() > MAX_NEW_STREAM_HANDLES_PER_VALUE {
            self.commit_resource_exhausted_terminal(index, stream_id, first_sequence)
                .await?;
            crate::metrics::durable_stream::record_limit_violation("streams_per_value");
            return Err(DurableStreamProducerError::ValueStreamLimit);
        }
        let mut new_streams_by_session = HashMap::<StreamSessionKeyV1, usize>::new();
        for request in &new_nested {
            let session_key = index
                .registration_session_key(&request.coordinate, &request.session_mapping)
                .ok_or_else(|| match &request.coordinate {
                    StreamRegistrationCoordinateV1::Nested {
                        parent_stream_id, ..
                    } => DurableStreamProducerError::UnknownStream(*parent_stream_id),
                    StreamRegistrationCoordinateV1::Root { .. } => {
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
            self.commit_resource_exhausted_terminal(index, stream_id, first_sequence)
                .await?;
            crate::metrics::durable_stream::record_limit_violation("streams_per_session");
            return Err(DurableStreamProducerError::StreamLimit);
        }
        if first_sequence.checked_add(item_count).is_none() {
            self.commit_resource_exhausted_terminal(index, stream_id, first_sequence)
                .await?;
            crate::metrics::durable_stream::record_limit_violation("sequence");
            return Err(DurableStreamProducerError::CounterOverflow);
        }
        let ingress_session_key = index
            .registrations
            .get(&stream_id)
            .filter(|registration| {
                registration.source_kind == StreamSourceKindV1::ExternalInlineInput
            })
            .and_then(|registration| registration.session_mapping.as_ref())
            .map(|mapping| mapping.session_key.clone());

        let environment_id = self.environment_id;
        let producer = self.producer.clone();
        let producer_fingerprint = self.producer_fingerprint;
        let payload_for_entry = payload.clone();
        let registrations_for_entry = new_nested;
        let nested_for_entry = nested_sources;
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
                    records.push(DurableStreamOplogRecord::Registered(registration));
                }
                let nested_stream_ids = nested_for_entry
                    .iter()
                    .map(|source| match source {
                        NestedStreamWriteV1::Register(request) => existing_nested
                            .get(&request.coordinate)
                            .or_else(|| newly_registered_by_coordinate.get(&request.coordinate))
                            .copied()
                            .expect(
                                "every validated nested stream is existing or newly registered",
                            ),
                        NestedStreamWriteV1::Forward(handle) => handle.stream_id,
                    })
                    .collect();
                let item_index =
                    OplogIndex::from_u64(first_index.as_u64() + registration_count as u64);
                let offsets = (0..payload_for_entry.logical_item_count())
                    .map(|sub_index| StreamOffsetV1::new(item_index, sub_index as u32))
                    .collect::<Vec<_>>();
                let payload_for_high_water = payload_for_entry.clone();
                records.push(DurableStreamOplogRecord::Items(StreamItemsRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    stream_id,
                    producer_fingerprint,
                    first_sequence,
                    nested_stream_ids,
                    newly_registered_stream_ids,
                    payload: payload_for_entry,
                    offsets,
                }));
                if let Some(session_key) = ingress_session_key {
                    let logical_item_count = payload_for_high_water.logical_item_count() as u64;
                    let resulting_offset = StreamOffsetV1::new(
                        item_index,
                        u32::try_from(logical_item_count - 1)
                            .expect("validated stream batch length fits in u32"),
                    );
                    records.push(DurableStreamOplogRecord::Session(Box::new(
                        StreamSessionRecordV1::InputHighWater(
                            StreamSessionInputHighWaterRecordV1 {
                                format_version: DURABLE_STREAM_FORMAT_VERSION,
                                session_key,
                                stream_id,
                                epoch: 1,
                                first_sequence,
                                payload: payload_for_high_water,
                                high_water: InputStreamHighWaterV1 {
                                    highest_contiguous_sequence: first_sequence
                                        + logical_item_count
                                        - 1,
                                    resulting_offset,
                                    terminal: false,
                                },
                            },
                        ),
                    )));
                }
                records
            }))
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        self.commit().await;

        let mut pending_registrations = Vec::new();
        let mut committed_item = None;
        for (oplog_index, entry) in entries {
            match entry {
                OplogEntry::StreamRegistered { record, .. } => {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(DurableStreamProducerError::Oplog)?;
                    pending_registrations.push((oplog_index, record));
                }
                OplogEntry::StreamItems { record, .. } => {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(DurableStreamProducerError::Oplog)?;
                    committed_item = Some((oplog_index, record));
                }
                OplogEntry::StreamSession { .. } => {}
                _ => unreachable!("stream item batch builder returned a different entry"),
            }
        }
        let (item_index, item_record) =
            committed_item.expect("stream item batch returned no item entry");
        let item_offsets = item_record.offsets.clone();
        let newly_registered_stream_ids = item_record.newly_registered_stream_ids.clone();
        let newly_registered_stream_count = newly_registered_stream_ids.len();
        let item_events = index.apply_item_batch(
            item_index,
            pending_registrations,
            item_record,
            self.environment_id,
            &self.producer,
            self.producer_fingerprint,
        )?;
        {
            let mut buses = self
                .buses
                .write()
                .expect("durable stream bus map lock poisoned");
            for nested_stream_id in newly_registered_stream_ids {
                buses.insert(
                    nested_stream_id,
                    Arc::new(DurableLiveStreamBus::new(self.live_join_capacity)?),
                );
            }
        }
        drop(index);
        let bus = self.bus(stream_id)?;
        for event in item_events {
            bus.publish_committed(DurableLiveStreamEvent {
                offset: event.offset,
                payload: event,
            })
            .await?;
        }
        self.record_registered_streams(newly_registered_stream_count);
        crate::metrics::durable_stream::record_producer_operation("write", false);
        tracing::debug!(
            stream_id = %stream_id,
            first_sequence,
            logical_item_count = item_count,
            nested_streams = newly_registered_stream_count,
            replayed = false,
            "Durable stream item batch committed"
        );
        Ok(ProducerWriteOutcomeV1 {
            value: item_offsets,
            replayed: false,
        })
    }

    async fn commit_resource_exhausted_terminal(
        &self,
        mut index: MutexGuard<'_, ProducerStreamIndex>,
        stream_id: StreamId,
        sequence: u64,
    ) -> Result<(), DurableStreamProducerError> {
        let result = StreamEndResultV1::ErrorContext(resource_exhausted_error_context()?);
        let producer_fingerprint = self.producer_fingerprint;
        let mut entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |oplog_index| {
                vec![DurableStreamOplogRecord::End(StreamEndRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    stream_id,
                    producer_fingerprint,
                    sequence,
                    offset: StreamOffsetV1::new(oplog_index, 0),
                    authored_by: StreamTerminalAuthorV1::Protocol,
                    result,
                })]
            }))
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        self.commit().await;
        let (oplog_index, entry) = entries
            .pop()
            .expect("resource exhaustion terminal batch returned no oplog entry");
        let OplogEntry::StreamEnd { record, .. } = entry else {
            unreachable!("resource exhaustion terminal builder returned a different entry")
        };
        let record = self
            .oplog
            .download_payload(record)
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        let event = index.apply_end(oplog_index, record, self.producer_fingerprint)?;
        drop(index);
        self.bus(stream_id)?
            .publish_committed(DurableLiveStreamEvent {
                offset: event.offset,
                payload: event,
            })
            .await?;
        self.record_terminal_streams(1);
        Ok(())
    }

    pub(crate) async fn end(
        &self,
        stream_id: StreamId,
        sequence: u64,
        result: StreamEndResultV1,
    ) -> Result<ProducerWriteOutcomeV1<StreamOffsetV1>, DurableStreamProducerError> {
        self.end_authored(stream_id, sequence, result, StreamTerminalAuthorV1::Guest)
            .await
    }

    async fn end_authored(
        &self,
        stream_id: StreamId,
        sequence: u64,
        result: StreamEndResultV1,
        authored_by: StreamTerminalAuthorV1,
    ) -> Result<ProducerWriteOutcomeV1<StreamOffsetV1>, DurableStreamProducerError> {
        let index = self.index.lock().await;
        self.end_authored_locked(index, stream_id, sequence, result, authored_by)
            .await
    }

    #[tracing::instrument(
        name = "durable_stream.end",
        skip_all,
        fields(stream_id = %stream_id, sequence)
    )]
    async fn end_authored_locked(
        &self,
        mut index: MutexGuard<'_, ProducerStreamIndex>,
        stream_id: StreamId,
        sequence: u64,
        result: StreamEndResultV1,
        authored_by: StreamTerminalAuthorV1,
    ) -> Result<ProducerWriteOutcomeV1<StreamOffsetV1>, DurableStreamProducerError> {
        match replay_terminal(
            &index,
            stream_id,
            sequence,
            &CommittedProducerStreamEventPayloadV1::End(result.clone()),
            authored_by,
        )? {
            TerminalReplayDecision::Append => {}
            TerminalReplayDecision::Replayed(event) => {
                let offset = event.offset;
                drop(index);
                self.publish_repair(stream_id, vec![event]).await?;
                crate::metrics::durable_stream::record_producer_operation("end", true);
                tracing::debug!(
                    stream_id = %stream_id,
                    sequence,
                    durable_offset = %offset,
                    replayed = true,
                    "Durable stream terminal resolved"
                );
                return Ok(ProducerWriteOutcomeV1 {
                    value: offset,
                    replayed: true,
                });
            }
            TerminalReplayDecision::Fenced(event) => {
                let error = DurableStreamProducerError::FencedByTerminal(event.payload.clone());
                drop(index);
                self.publish_repair(stream_id, vec![event]).await?;
                return Err(error);
            }
        }
        index.ensure_producer_write_allowed()?;
        let stream = index
            .streams
            .get(&stream_id)
            .expect("terminal replay validated the stream");
        if stream.terminal {
            let terminal = stream
                .events
                .values()
                .next_back()
                .expect("terminal stream has no terminal event")
                .clone();
            let error = fenced_by_terminal(stream);
            drop(index);
            self.publish_repair(stream_id, vec![terminal]).await?;
            return Err(error);
        }
        validate_new_terminal(&index, stream_id, sequence)?;
        let producer_fingerprint = self.producer_fingerprint;
        let mut entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |oplog_index| {
                vec![DurableStreamOplogRecord::End(StreamEndRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    stream_id,
                    producer_fingerprint,
                    sequence,
                    offset: StreamOffsetV1::new(oplog_index, 0),
                    authored_by,
                    result,
                })]
            }))
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        self.commit().await;
        let (oplog_index, entry) = entries
            .pop()
            .expect("stream end batch returned no oplog entry");
        let OplogEntry::StreamEnd { record, .. } = entry else {
            unreachable!("stream end builder returned a different entry")
        };
        let record = self
            .oplog
            .download_payload(record)
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        let event = index.apply_end(oplog_index, record, self.producer_fingerprint)?;
        let offset = event.offset;
        drop(index);
        self.bus(stream_id)?
            .publish_committed(DurableLiveStreamEvent {
                offset,
                payload: event,
            })
            .await?;
        self.record_terminal_streams(1);
        crate::metrics::durable_stream::record_producer_operation("end", false);
        tracing::debug!(
            stream_id = %stream_id,
            sequence,
            durable_offset = %offset,
            replayed = false,
            "Durable stream terminal committed"
        );
        Ok(ProducerWriteOutcomeV1 {
            value: offset,
            replayed: false,
        })
    }

    #[tracing::instrument(
        name = "durable_stream.cancel",
        skip_all,
        fields(stream_id = %stream_id, sequence, role = ?role, reason = ?reason)
    )]
    async fn cancel_locked(
        &self,
        mut index: MutexGuard<'_, ProducerStreamIndex>,
        stream_id: StreamId,
        sequence: u64,
        role: StreamCancelRoleV1,
        reason: StreamCancelReasonV1,
        details: Option<String>,
    ) -> Result<ProducerWriteOutcomeV1<StreamOffsetV1>, DurableStreamProducerError> {
        let payload = CommittedProducerStreamEventPayloadV1::Cancel {
            role,
            reason,
            details: details.clone(),
        };
        match replay_terminal(
            &index,
            stream_id,
            sequence,
            &payload,
            StreamTerminalAuthorV1::Protocol,
        )? {
            TerminalReplayDecision::Append => {}
            TerminalReplayDecision::Replayed(event) => {
                let offset = event.offset;
                drop(index);
                self.cancel_source(stream_id);
                self.publish_repair(stream_id, vec![event]).await?;
                crate::metrics::durable_stream::record_producer_operation("cancel", true);
                tracing::debug!(
                    stream_id = %stream_id,
                    sequence,
                    durable_offset = %offset,
                    role = ?role,
                    reason = ?reason,
                    replayed = true,
                    "Durable stream cancellation resolved"
                );
                return Ok(ProducerWriteOutcomeV1 {
                    value: offset,
                    replayed: true,
                });
            }
            TerminalReplayDecision::Fenced(event) => {
                let error = DurableStreamProducerError::FencedByTerminal(event.payload.clone());
                drop(index);
                self.cancel_source(stream_id);
                self.publish_repair(stream_id, vec![event]).await?;
                return Err(error);
            }
        }
        index.ensure_producer_write_allowed()?;
        validate_new_terminal(&index, stream_id, sequence)?;
        let producer_fingerprint = self.producer_fingerprint;
        let mut entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |oplog_index| {
                vec![DurableStreamOplogRecord::Cancel(StreamCancelRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    stream_id,
                    producer_fingerprint,
                    sequence,
                    offset: StreamOffsetV1::new(oplog_index, 0),
                    authored_by: StreamTerminalAuthorV1::Protocol,
                    role,
                    reason,
                    details,
                })]
            }))
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        self.commit().await;
        let (oplog_index, entry) = entries
            .pop()
            .expect("stream cancellation batch returned no oplog entry");
        let OplogEntry::StreamCancel { record, .. } = entry else {
            unreachable!("stream cancellation builder returned a different entry")
        };
        let record = self
            .oplog
            .download_payload(record)
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        let event = index.apply_cancel(oplog_index, record, self.producer_fingerprint)?;
        let offset = event.offset;
        drop(index);
        self.cancel_source(stream_id);
        self.bus(stream_id)?
            .publish_committed(DurableLiveStreamEvent {
                offset,
                payload: event,
            })
            .await?;
        self.record_terminal_streams(1);
        crate::metrics::durable_stream::record_producer_operation("cancel", false);
        tracing::debug!(
            stream_id = %stream_id,
            sequence,
            durable_offset = %offset,
            role = ?role,
            reason = ?reason,
            replayed = false,
            "Durable stream cancellation committed"
        );
        Ok(ProducerWriteOutcomeV1 {
            value: offset,
            replayed: false,
        })
    }

    pub(crate) async fn cancel_open(
        &self,
        stream_id: StreamId,
        role: StreamCancelRoleV1,
        reason: StreamCancelReasonV1,
        details: Option<String>,
    ) -> Result<(), DurableStreamProducerError> {
        let index = self.index.lock().await;
        let stream = index
            .streams
            .get(&stream_id)
            .ok_or(DurableStreamProducerError::UnknownStream(stream_id))?;
        if stream.terminal {
            drop(index);
            self.cancel_source(stream_id);
            return Ok(());
        }
        let sequence = stream.next_sequence;
        self.cancel_locked(index, stream_id, sequence, role, reason, details)
            .await?;
        Ok(())
    }

    pub(crate) fn register_source_cancellation(
        &self,
        stream_id: StreamId,
        cancellation: CancellationToken,
    ) -> u64 {
        let registration_id = self
            .next_source_cancellation_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .expect("durable stream source cancellation registration IDs exhausted");
        let replaced = self
            .source_cancellations
            .write()
            .expect("durable stream source cancellation lock poisoned")
            .insert(stream_id, (registration_id, cancellation));
        if let Some((_, replaced)) = replaced {
            replaced.cancel();
        }
        registration_id
    }

    pub(crate) fn unregister_source_cancellation(&self, stream_id: StreamId, registration_id: u64) {
        let mut registrations = self
            .source_cancellations
            .write()
            .expect("durable stream source cancellation lock poisoned");
        if registrations
            .get(&stream_id)
            .is_some_and(|(current_id, _)| *current_id == registration_id)
        {
            registrations.remove(&stream_id);
        }
    }

    fn cancel_source(&self, stream_id: StreamId) {
        if let Some(cancellation) = self
            .source_cancellations
            .read()
            .expect("durable stream source cancellation lock poisoned")
            .get(&stream_id)
        {
            cancellation.1.cancel();
        }
    }

    pub(crate) async fn end_open(
        &self,
        stream_id: StreamId,
        result: StreamEndResultV1,
    ) -> Result<(), DurableStreamProducerError> {
        let index = self.index.lock().await;
        let stream = index
            .streams
            .get(&stream_id)
            .ok_or(DurableStreamProducerError::UnknownStream(stream_id))?;
        if stream.terminal {
            return Ok(());
        }
        let sequence = stream.next_sequence;
        self.end_authored_locked(
            index,
            stream_id,
            sequence,
            result,
            StreamTerminalAuthorV1::Protocol,
        )
        .await?;
        Ok(())
    }

    pub(crate) async fn has_open_forwarded_session_input(
        &self,
        session_key: &StreamSessionKeyV1,
    ) -> bool {
        let index = self.index.lock().await;
        let Some(mappings) = index.session_stream_mappings.get(session_key) else {
            return false;
        };
        mappings.iter().any(|(handle, role)| {
            *role == SessionStreamRoleV1::Input
                && mappings.contains(&(handle.clone(), SessionStreamRoleV1::Output))
                && index
                    .streams
                    .get(&handle.stream_id)
                    .is_some_and(|stream| !stream.terminal)
        })
    }

    pub(crate) async fn finish_session(
        &self,
        session_key: StreamSessionKeyV1,
        result: Result<(), Vec<u8>>,
        input_cancel_reason: StreamCancelReasonV1,
    ) -> Result<(), DurableStreamProducerError> {
        let mut index = self.index.lock().await;
        if index.finished_sessions.contains(&session_key) {
            return Ok(());
        }
        index.ensure_producer_write_allowed()?;
        let mut open_streams = index
            .stream_sessions
            .iter()
            .filter_map(|(stream_id, candidate_session)| {
                if candidate_session != &session_key {
                    return None;
                }
                let stream = index
                    .streams
                    .get(stream_id)
                    .expect("session stream index points at a missing stream");
                (!stream.terminal).then(|| {
                    (
                        *stream_id,
                        *index
                            .stream_roles
                            .get(stream_id)
                            .expect("session stream index points at a missing role"),
                        stream.next_sequence,
                    )
                })
            })
            .collect::<Vec<_>>();
        open_streams.sort_by_key(|(stream_id, _, _)| *stream_id);

        let producer_fingerprint = self.producer_fingerprint;
        let result_for_batch = result.clone();
        let session_key_for_batch = session_key.clone();
        let entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |first_index| {
                let mut records = Vec::with_capacity(open_streams.len() + 1);
                for (position, (stream_id, role, sequence)) in open_streams.into_iter().enumerate()
                {
                    let oplog_index = OplogIndex::from_u64(first_index.as_u64() + position as u64);
                    match role {
                        SessionStreamRoleV1::Input => {
                            records.push(DurableStreamOplogRecord::Cancel(StreamCancelRecordV1 {
                                format_version: DURABLE_STREAM_FORMAT_VERSION,
                                stream_id,
                                producer_fingerprint,
                                sequence,
                                offset: StreamOffsetV1::new(oplog_index, 0),
                                authored_by: StreamTerminalAuthorV1::Protocol,
                                role: StreamCancelRoleV1::InputConsumer,
                                reason: input_cancel_reason,
                                details: Some(
                                    "invocation finished before consuming the complete input"
                                        .to_string(),
                                ),
                            }));
                        }
                        SessionStreamRoleV1::Output => {
                            let details = match &result_for_batch {
                                Ok(()) => b"output stream ended without a terminal".to_vec(),
                                Err(details) => details.clone(),
                            };
                            records.push(DurableStreamOplogRecord::End(StreamEndRecordV1 {
                                format_version: DURABLE_STREAM_FORMAT_VERSION,
                                stream_id,
                                producer_fingerprint,
                                sequence,
                                offset: StreamOffsetV1::new(oplog_index, 0),
                                authored_by: StreamTerminalAuthorV1::Protocol,
                                result: StreamEndResultV1::ErrorContext(details),
                            }));
                        }
                    }
                }
                records.push(DurableStreamOplogRecord::Session(Box::new(
                    StreamSessionRecordV1::Finished(StreamSessionFinishedRecordV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: session_key_for_batch,
                        result: result_for_batch,
                    }),
                )));
                records
            }))
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        self.commit().await;

        let mut terminal_events = Vec::new();
        for (oplog_index, entry) in entries {
            match entry {
                OplogEntry::StreamEnd { record, .. } => {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(DurableStreamProducerError::Oplog)?;
                    terminal_events.push(index.apply_end(
                        oplog_index,
                        record,
                        self.producer_fingerprint,
                    )?);
                }
                OplogEntry::StreamCancel { record, .. } => {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(DurableStreamProducerError::Oplog)?;
                    terminal_events.push(index.apply_cancel(
                        oplog_index,
                        record,
                        self.producer_fingerprint,
                    )?);
                }
                OplogEntry::StreamSession { record, .. } => {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(DurableStreamProducerError::Oplog)?;
                    let StreamSessionRecordV1::Finished(record) = record else {
                        return Err(DurableStreamProducerError::CorruptHistory(
                            "session finish batch contains an unexpected session record"
                                .to_string(),
                        ));
                    };
                    index.apply_finished(&record)?;
                }
                _ => {
                    return Err(DurableStreamProducerError::CorruptHistory(
                        "session finish batch contains an unexpected oplog entry".to_string(),
                    ));
                }
            }
        }
        drop(index);
        let terminal_count = terminal_events.len();
        for event in terminal_events {
            self.bus(event.stream_id)?
                .publish_committed(DurableLiveStreamEvent {
                    offset: event.offset,
                    payload: event,
                })
                .await?;
        }
        self.record_terminal_streams(terminal_count);
        crate::metrics::durable_stream::record_producer_operation("finish_session", false);
        tracing::debug!(
            terminal_streams = terminal_count,
            "Durable Stream Session finish committed"
        );
        self.notify_session_records_changed();
        Ok(())
    }

    #[tracing::instrument(
        name = "durable_stream.catch_up",
        skip_all,
        fields(stream_id = %handle.stream_id, has_cursor = after.is_some())
    )]
    pub(crate) async fn catch_up(
        &self,
        handle: DurableStreamHandleV1,
        after: Option<StreamOffsetV1>,
    ) -> Result<DurableCatchUpReader, DurableStreamProducerError> {
        self.validate_handle(&handle).await?;
        self.validate_cursor(handle.stream_id, after).await?;
        let bus = self.bus(handle.stream_id)?;
        let subscription = bus.subscribe().await?;
        let history = match subscription.high_water {
            Some(high_water) => match self.read_segment(&handle, after, Some(high_water)).await {
                Ok(history) => history,
                Err(error) => {
                    bus.unsubscribe(subscription.reader_id()).await;
                    crate::metrics::durable_stream::record_live_join_rejected();
                    return Err(error);
                }
            },
            None => Vec::new(),
        };
        let join_high_water = subscription.high_water;
        crate::metrics::durable_stream::record_catch_up(history.len());
        tracing::debug!(
            stream_id = %handle.stream_id,
            catch_up_events = history.len(),
            has_cursor = after.is_some(),
            has_join_high_water = join_high_water.is_some(),
            "Durable stream reader joined committed history to live tail"
        );
        Ok(DurableCatchUpReader {
            bus,
            subscription: Some(subscription),
            history: history.into(),
            join_high_water,
            last_delivered: after,
            terminal_delivered: false,
        })
    }

    pub(crate) async fn validate_handle(
        &self,
        handle: &DurableStreamHandleV1,
    ) -> Result<(), DurableStreamProducerError> {
        validate_version(handle.format_version)?;
        let index = self.index.lock().await;
        if index
            .registrations
            .get(&handle.stream_id)
            .is_some_and(|record| &record.handle == handle)
        {
            Ok(())
        } else {
            Err(DurableStreamProducerError::InvalidHandle)
        }
    }

    pub(crate) fn owns_handle_identity(&self, handle: &DurableStreamHandleV1) -> bool {
        handle.producer_environment_id == self.environment_id
            && handle.producer == self.producer
            && handle.expected_producer_fingerprint == self.producer_fingerprint
    }

    async fn validate_cursor(
        &self,
        stream_id: StreamId,
        after: Option<StreamOffsetV1>,
    ) -> Result<(), DurableStreamProducerError> {
        let Some(after) = after else {
            return Ok(());
        };
        let index = self.index.lock().await;
        if index
            .streams
            .get(&stream_id)
            .is_some_and(|stream| stream.events.values().any(|event| event.offset == after))
        {
            Ok(())
        } else {
            Err(DurableStreamProducerError::CursorUnavailable)
        }
    }

    fn bus(
        &self,
        stream_id: StreamId,
    ) -> Result<Arc<DurableLiveStreamBus<CommittedProducerStreamEventV1>>, DurableStreamProducerError>
    {
        self.buses
            .read()
            .expect("durable stream bus map lock poisoned")
            .get(&stream_id)
            .cloned()
            .ok_or(DurableStreamProducerError::UnknownStream(stream_id))
    }

    async fn publish_repair(
        &self,
        stream_id: StreamId,
        events: Vec<CommittedProducerStreamEventV1>,
    ) -> Result<(), DurableStreamProducerError> {
        let bus = self.bus(stream_id)?;
        for event in events {
            bus.republish_committed(DurableLiveStreamEvent {
                offset: event.offset,
                payload: event,
            })
            .await;
        }
        Ok(())
    }
}

impl Drop for DurableStreamProducer {
    fn drop(&mut self) {
        crate::metrics::durable_stream::remove_open_streams(
            self.open_stream_count.load(Ordering::Relaxed),
        );
    }
}

fn registration_record(
    oplog_index: OplogIndex,
    environment_id: EnvironmentId,
    producer: AgentId,
    producer_fingerprint: AgentFingerprint,
    request: ProducerRegistrationRequestV1,
) -> StreamRegisteredRecordV1 {
    let stream_id = StreamId::derive(environment_id, &producer, producer_fingerprint, oplog_index)
        .expect("producer identity was validated before reserving the registration index");
    StreamRegisteredRecordV1 {
        format_version: DURABLE_STREAM_FORMAT_VERSION,
        coordinate: request.coordinate,
        registration_oplog_index: oplog_index,
        handle: DurableStreamHandleV1 {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            stream_id,
            producer_environment_id: environment_id,
            producer,
            expected_producer_fingerprint: producer_fingerprint,
            source_invocation: request.source_invocation,
            component_revision: request.component_revision,
            element_schema_fingerprint: request.element_schema_fingerprint,
        },
        source_kind: request.source_kind,
        session_mapping: request.session_mapping,
    }
}

fn registration_matches(
    record: &StreamRegisteredRecordV1,
    request: &ProducerRegistrationRequestV1,
) -> bool {
    record.coordinate == request.coordinate
        && record.handle.source_invocation == request.source_invocation
        && record.handle.component_revision == request.component_revision
        && record.handle.element_schema_fingerprint == request.element_schema_fingerprint
        && record.source_kind == request.source_kind
        && record.session_mapping == request.session_mapping
}

enum TerminalReplayDecision {
    Append,
    Replayed(CommittedProducerStreamEventV1),
    Fenced(CommittedProducerStreamEventV1),
}

fn replay_terminal(
    index: &ProducerStreamIndex,
    stream_id: StreamId,
    sequence: u64,
    expected_payload: &CommittedProducerStreamEventPayloadV1,
    expected_author: StreamTerminalAuthorV1,
) -> Result<TerminalReplayDecision, DurableStreamProducerError> {
    let stream = index
        .streams
        .get(&stream_id)
        .ok_or(DurableStreamProducerError::UnknownStream(stream_id))?;
    if let Some(event) = stream.events.get(&sequence) {
        if event.is_terminal()
            && event.terminal_author == Some(expected_author)
            && &event.payload == expected_payload
        {
            return Ok(TerminalReplayDecision::Replayed(event.clone()));
        }
        if event.is_terminal()
            && event.terminal_author == Some(StreamTerminalAuthorV1::Protocol)
            && expected_author == StreamTerminalAuthorV1::Guest
        {
            return Ok(TerminalReplayDecision::Fenced(event.clone()));
        }
        return Err(DurableStreamProducerError::EventConflict);
    }
    Ok(TerminalReplayDecision::Append)
}

fn validate_new_terminal(
    index: &ProducerStreamIndex,
    stream_id: StreamId,
    sequence: u64,
) -> Result<(), DurableStreamProducerError> {
    let stream = index
        .streams
        .get(&stream_id)
        .ok_or(DurableStreamProducerError::UnknownStream(stream_id))?;
    validate_terminal_sequence(stream, stream_id, sequence)
}

fn fenced_by_terminal(stream: &IndexedProducerStream) -> DurableStreamProducerError {
    DurableStreamProducerError::FencedByTerminal(
        stream
            .events
            .values()
            .next_back()
            .expect("terminal stream has no terminal event")
            .payload
            .clone(),
    )
}

#[async_trait]
pub(crate) trait StreamSegmentSource: Send + Sync {
    async fn read_segment(
        &self,
        handle: &DurableStreamHandleV1,
        after: Option<StreamOffsetV1>,
        through: Option<StreamOffsetV1>,
    ) -> Result<Vec<CommittedProducerStreamEventV1>, DurableStreamProducerError>;
}

#[async_trait]
pub(crate) trait AttachedStreamSegmentSource: Send + Sync {
    async fn read_attached_segment(
        &self,
        attachment: &StreamAttachmentKeyV1,
        handle: &DurableStreamHandleV1,
        now_millis: u64,
        after: Option<StreamOffsetV1>,
        through: Option<StreamOffsetV1>,
    ) -> Result<Vec<CommittedProducerStreamEventV1>, DurableStreamProducerError>;

    async fn wait_for_attached_segment(
        &self,
        attachment: &StreamAttachmentKeyV1,
        handle: &DurableStreamHandleV1,
        now_millis: u64,
        after: Option<StreamOffsetV1>,
    ) -> Result<Vec<CommittedProducerStreamEventV1>, DurableStreamProducerError>;
}

#[async_trait]
pub(crate) trait StreamAttachmentControl: Send + Sync {
    async fn prepare_attachment(
        &self,
        key: StreamAttachmentKeyV1,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcomeV1<StreamAttachmentViewV1>, DurableStreamProducerError>;

    async fn activate_attachment(
        &self,
        key: StreamAttachmentKeyV1,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcomeV1<StreamAttachmentViewV1>, DurableStreamProducerError>;

    async fn detach_attachment(
        &self,
        key: &StreamAttachmentKeyV1,
    ) -> Result<StreamAttachmentViewV1, DurableStreamProducerError>;

    async fn renew_attachment(
        &self,
        key: StreamAttachmentKeyV1,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcomeV1<StreamAttachmentViewV1>, DurableStreamProducerError>;

    async fn finalize_attachment(
        &self,
        key: StreamAttachmentKeyV1,
        reason: StreamAttachmentFinalizationReasonV1,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcomeV1<StreamAttachmentViewV1>, DurableStreamProducerError>;

    async fn inspect_attachments(&self) -> Vec<StreamAttachmentViewV1>;
}

pub(crate) struct RoutedStreamAttachmentControl {
    rpc: Arc<dyn Rpc>,
    mapping: StreamSessionMappingRecordV1,
    auth_ctx: AuthCtx,
}

pub(crate) struct RoutedAttachedStreamSegmentSource {
    rpc: Arc<dyn Rpc>,
    mapping: StreamSessionMappingRecordV1,
    auth_ctx: AuthCtx,
}

impl RoutedAttachedStreamSegmentSource {
    pub(crate) fn new(
        rpc: Arc<dyn Rpc>,
        mapping: StreamSessionMappingRecordV1,
        auth_ctx: AuthCtx,
    ) -> Self {
        Self {
            rpc,
            mapping,
            auth_ctx,
        }
    }
}

#[async_trait]
impl AttachedStreamSegmentSource for RoutedAttachedStreamSegmentSource {
    async fn read_attached_segment(
        &self,
        attachment: &StreamAttachmentKeyV1,
        handle: &DurableStreamHandleV1,
        _now_millis: u64,
        after: Option<StreamOffsetV1>,
        through: Option<StreamOffsetV1>,
    ) -> Result<Vec<CommittedProducerStreamEventV1>, DurableStreamProducerError> {
        if self.mapping.handle != *handle {
            return Err(DurableStreamProducerError::InvalidHandle);
        }
        let payload = self
            .rpc
            .read_durable_stream_segment(
                AttachedStreamSegmentRequestV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    attachment: attachment.clone(),
                    mapping: self.mapping.clone(),
                    after,
                    through,
                    wait_for_events: false,
                },
                &self.auth_ctx,
            )
            .await
            .map_err(|error| DurableStreamProducerError::Oplog(error.to_string()))?;
        golem_common::serialization::deserialize(&payload)
            .map_err(DurableStreamProducerError::CorruptHistory)
    }

    async fn wait_for_attached_segment(
        &self,
        attachment: &StreamAttachmentKeyV1,
        handle: &DurableStreamHandleV1,
        _now_millis: u64,
        after: Option<StreamOffsetV1>,
    ) -> Result<Vec<CommittedProducerStreamEventV1>, DurableStreamProducerError> {
        if self.mapping.handle != *handle {
            return Err(DurableStreamProducerError::InvalidHandle);
        }
        let payload = self
            .rpc
            .read_durable_stream_segment(
                AttachedStreamSegmentRequestV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    attachment: attachment.clone(),
                    mapping: self.mapping.clone(),
                    after,
                    through: None,
                    wait_for_events: true,
                },
                &self.auth_ctx,
            )
            .await
            .map_err(|error| DurableStreamProducerError::Oplog(error.to_string()))?;
        golem_common::serialization::deserialize(&payload)
            .map_err(DurableStreamProducerError::CorruptHistory)
    }
}

impl RoutedStreamAttachmentControl {
    pub(crate) fn new(
        rpc: Arc<dyn Rpc>,
        mapping: StreamSessionMappingRecordV1,
        auth_ctx: AuthCtx,
    ) -> Self {
        Self {
            rpc,
            mapping,
            auth_ctx,
        }
    }

    async fn execute(
        &self,
        operation: StreamAttachmentControlOperationV1,
    ) -> Result<bool, DurableStreamProducerError> {
        self.rpc
            .control_durable_stream_attachment(
                StreamAttachmentControlRequestV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    mapping: Some(self.mapping.clone()),
                    operation,
                },
                &self.auth_ctx,
            )
            .await
            .map_err(|error| DurableStreamProducerError::Oplog(error.to_string()))
    }

    pub(crate) async fn cancel_stream(
        &self,
        key: StreamAttachmentKeyV1,
        role: StreamCancelRoleV1,
        reason: StreamCancelReasonV1,
        details: Option<String>,
    ) -> Result<bool, DurableStreamProducerError> {
        self.execute(StreamAttachmentControlOperationV1::Cancel {
            key,
            role,
            reason,
            details,
        })
        .await
    }
}

#[async_trait]
impl StreamAttachmentControl for RoutedStreamAttachmentControl {
    async fn prepare_attachment(
        &self,
        key: StreamAttachmentKeyV1,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcomeV1<StreamAttachmentViewV1>, DurableStreamProducerError> {
        let replayed = self
            .execute(StreamAttachmentControlOperationV1::Prepare {
                key: key.clone(),
                now_millis,
            })
            .await?;
        Ok(ProducerWriteOutcomeV1 {
            value: StreamAttachmentViewV1 {
                key,
                state: StreamAttachmentStateV1::Prepared,
                lease_expires_at_millis: Some(attachment_lease_expiry(now_millis)?),
            },
            replayed,
        })
    }

    async fn activate_attachment(
        &self,
        key: StreamAttachmentKeyV1,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcomeV1<StreamAttachmentViewV1>, DurableStreamProducerError> {
        let replayed = self
            .execute(StreamAttachmentControlOperationV1::Activate {
                key: key.clone(),
                now_millis,
            })
            .await?;
        Ok(ProducerWriteOutcomeV1 {
            value: StreamAttachmentViewV1 {
                key,
                state: StreamAttachmentStateV1::Active,
                lease_expires_at_millis: Some(attachment_lease_expiry(now_millis)?),
            },
            replayed,
        })
    }

    async fn detach_attachment(
        &self,
        key: &StreamAttachmentKeyV1,
    ) -> Result<StreamAttachmentViewV1, DurableStreamProducerError> {
        self.execute(StreamAttachmentControlOperationV1::Detach { key: key.clone() })
            .await?;
        Ok(StreamAttachmentViewV1 {
            key: key.clone(),
            state: StreamAttachmentStateV1::Active,
            lease_expires_at_millis: None,
        })
    }

    async fn renew_attachment(
        &self,
        key: StreamAttachmentKeyV1,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcomeV1<StreamAttachmentViewV1>, DurableStreamProducerError> {
        let replayed = self
            .execute(StreamAttachmentControlOperationV1::Renew {
                key: key.clone(),
                now_millis,
            })
            .await?;
        Ok(ProducerWriteOutcomeV1 {
            value: StreamAttachmentViewV1 {
                key,
                state: StreamAttachmentStateV1::Active,
                lease_expires_at_millis: Some(attachment_lease_expiry(now_millis)?),
            },
            replayed,
        })
    }

    async fn finalize_attachment(
        &self,
        key: StreamAttachmentKeyV1,
        reason: StreamAttachmentFinalizationReasonV1,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcomeV1<StreamAttachmentViewV1>, DurableStreamProducerError> {
        let replayed = self
            .execute(StreamAttachmentControlOperationV1::Finalize {
                key: key.clone(),
                reason,
                now_millis,
            })
            .await?;
        Ok(ProducerWriteOutcomeV1 {
            value: StreamAttachmentViewV1 {
                key,
                state: StreamAttachmentStateV1::Finalized(reason),
                lease_expires_at_millis: None,
            },
            replayed,
        })
    }

    async fn inspect_attachments(&self) -> Vec<StreamAttachmentViewV1> {
        Vec::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConsumerAttachmentStatus {
    Prepared,
    Active,
    Deleting,
    Missing,
    IncarnationMismatch,
    EpochMismatch,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ConsumerJournalInspection {
    pub(crate) source_offsets: Vec<StreamOffsetV1>,
    pub(crate) source_unavailable: Option<StreamOffsetV1>,
}

#[async_trait]
pub(crate) trait StreamAttachmentConsumerProbe: Send + Sync {
    async fn status(
        &self,
        key: &StreamAttachmentKeyV1,
    ) -> Result<ConsumerAttachmentStatus, DurableStreamProducerError>;

    async fn status_exact(
        &self,
        key: &StreamAttachmentKeyV1,
        _mapping: Option<&StreamSessionMappingRecordV1>,
    ) -> Result<ConsumerAttachmentStatus, DurableStreamProducerError> {
        self.status(key).await
    }

    async fn journal_inspection(
        &self,
        _key: &StreamAttachmentKeyV1,
    ) -> Result<Option<ConsumerJournalInspection>, DurableStreamProducerError> {
        Ok(None)
    }

    async fn commit_source_unavailable(
        &self,
        _key: &StreamAttachmentKeyV1,
        _source_offset: StreamOffsetV1,
        _consumer_read_ordinal: u64,
    ) -> Result<(), DurableStreamProducerError> {
        Err(DurableStreamProducerError::Oplog(
            "consumer probe cannot commit a source-unavailable overlay".to_string(),
        ))
    }
}

pub(crate) struct DbDirectStreamAttachmentConsumerProbe {
    worker_service: Arc<dyn WorkerService>,
    oplog_service: Arc<dyn OplogService>,
    rpc: Option<Arc<dyn Rpc>>,
}

impl DbDirectStreamAttachmentConsumerProbe {
    pub(crate) fn new(
        worker_service: Arc<dyn WorkerService>,
        oplog_service: Arc<dyn OplogService>,
    ) -> Self {
        Self {
            worker_service,
            oplog_service,
            rpc: None,
        }
    }

    pub(crate) fn new_routed(
        worker_service: Arc<dyn WorkerService>,
        oplog_service: Arc<dyn OplogService>,
        rpc: Arc<dyn Rpc>,
    ) -> Self {
        Self {
            worker_service,
            oplog_service,
            rpc: Some(rpc),
        }
    }
}

#[async_trait]
impl StreamAttachmentConsumerProbe for DbDirectStreamAttachmentConsumerProbe {
    async fn status(
        &self,
        key: &StreamAttachmentKeyV1,
    ) -> Result<ConsumerAttachmentStatus, DurableStreamProducerError> {
        self.status_exact(key, None).await
    }

    async fn status_exact(
        &self,
        key: &StreamAttachmentKeyV1,
        expected_mapping: Option<&StreamSessionMappingRecordV1>,
    ) -> Result<ConsumerAttachmentStatus, DurableStreamProducerError> {
        let session_owner = OwnedAgentId::new(
            key.session_key.callee_environment_id,
            &key.session_key.callee,
        );
        let Some(session_metadata) = self.worker_service.get(&session_owner).await else {
            return Ok(ConsumerAttachmentStatus::Missing);
        };
        if session_metadata.initial_worker_metadata.fingerprint
            != key.session_key.callee_fingerprint
            || session_metadata.initial_worker_metadata.agent_mode != AgentMode::Durable
        {
            return Ok(ConsumerAttachmentStatus::IncarnationMismatch);
        }
        let session_mode = session_metadata.initial_worker_metadata.agent_mode;
        let session_current = self
            .oplog_service
            .get_last_index(&session_owner, session_mode)
            .await;
        if !session_current.is_defined() {
            return Ok(ConsumerAttachmentStatus::Missing);
        }
        let mut prepared_attempt_id = None;
        let mut initial_attachment = None;
        let mut attachment_authority = None;
        let mut pending_invocations = HashMap::new();
        for (oplog_index, entry) in self
            .oplog_service
            .read_exact(
                &session_owner,
                session_mode,
                OplogIndex::INITIAL,
                session_current.as_u64(),
            )
            .await
        {
            if let OplogEntry::PendingAgentInvocation {
                idempotency_key, ..
            } = &entry
            {
                pending_invocations.insert(oplog_index, idempotency_key.clone());
            }
            let OplogEntry::StreamSession { record, .. } = entry else {
                continue;
            };
            let record = self
                .oplog_service
                .download_payload(&session_owner, session_mode, record)
                .await
                .map_err(DurableStreamProducerError::Oplog)?;
            if !record.has_supported_format() {
                return Err(DurableStreamProducerError::CorruptHistory(
                    "unsupported or malformed durable Stream Session record".to_string(),
                ));
            }
            match record {
                StreamSessionRecordV1::Prepared(record)
                    if record.attempt.session_key == key.session_key =>
                {
                    if record.attempt.attachment_id != key.attachment_id
                        || record.attempt.expected_callee_fingerprint
                            != key.session_key.callee_fingerprint
                        || record.attempt.invocation.session_key != key.session_key
                    {
                        return Ok(ConsumerAttachmentStatus::IncarnationMismatch);
                    }
                    if prepared_attempt_id
                        .replace(record.attempt.attempt_id)
                        .is_some()
                    {
                        return Err(DurableStreamProducerError::CorruptHistory(
                            "durable Stream Session contains multiple Prepared records".to_string(),
                        ));
                    }
                }
                StreamSessionRecordV1::Attached(record)
                    if record.session_key == key.session_key =>
                {
                    if record.attachment_id != key.attachment_id {
                        return Ok(ConsumerAttachmentStatus::IncarnationMismatch);
                    }
                    if initial_attachment.replace(record.clone()).is_some()
                        || attachment_authority.is_some()
                    {
                        return Err(DurableStreamProducerError::CorruptHistory(
                            "durable Stream Session contains multiple Attached records".to_string(),
                        ));
                    }
                    attachment_authority = Some((record.epoch, record.attempt_id, true));
                }
                StreamSessionRecordV1::ResumeAttempt(record)
                    if record.attempt.session_key == key.session_key =>
                {
                    if record.attempt.attachment_id != key.attachment_id {
                        return Ok(ConsumerAttachmentStatus::IncarnationMismatch);
                    }
                    let Some((epoch, _, _)) = attachment_authority else {
                        return Err(DurableStreamProducerError::CorruptHistory(
                            "durable resume precedes initial attachment".to_string(),
                        ));
                    };
                    if record.attempt.expected_epoch != epoch
                        || record.accepted_epoch != epoch.checked_add(1).unwrap_or_default()
                    {
                        return Err(DurableStreamProducerError::CorruptHistory(
                            "durable resume contains an invalid epoch transition".to_string(),
                        ));
                    }
                    attachment_authority =
                        Some((record.accepted_epoch, record.attempt.attempt_id, true));
                }
                StreamSessionRecordV1::Detached(record)
                    if record.session_key == key.session_key =>
                {
                    let Some((epoch, attempt_id, attached)) = attachment_authority else {
                        return Err(DurableStreamProducerError::CorruptHistory(
                            "durable detach precedes initial attachment".to_string(),
                        ));
                    };
                    if record.attachment_id != key.attachment_id
                        || record.epoch != epoch
                        || record.owner_attempt_id != attempt_id
                    {
                        return Err(DurableStreamProducerError::CorruptHistory(
                            "durable detach does not match the current attachment".to_string(),
                        ));
                    }
                    if attached {
                        attachment_authority = Some((epoch, attempt_id, false));
                    }
                }
                _ => {}
            }
        }
        let Some(prepared_attempt_id) = prepared_attempt_id else {
            return Ok(ConsumerAttachmentStatus::Missing);
        };
        if let Some(attached) = &initial_attachment
            && (attached.attempt_id != prepared_attempt_id
                || pending_invocations.get(&attached.pending_invocation_oplog_index)
                    != Some(&key.session_key.idempotency_key))
        {
            return Err(DurableStreamProducerError::CorruptHistory(
                    "durable Attached record does not identify its Prepared attempt and pending invocation"
                        .to_string(),
                ));
        }
        if attachment_authority.is_some_and(|(epoch, _, _)| epoch != key.epoch) {
            return Ok(ConsumerAttachmentStatus::EpochMismatch);
        }

        let consumer = OwnedAgentId::new(key.consumer_environment_id, &key.consumer);
        let Some(metadata) = self.worker_service.get(&consumer).await else {
            return Ok(ConsumerAttachmentStatus::Missing);
        };
        if metadata.initial_worker_metadata.fingerprint != key.expected_consumer_fingerprint
            || key.consumer_invocation.callee_environment_id != key.consumer_environment_id
            || key.consumer_invocation.callee != key.consumer
            || key.consumer_invocation.callee_fingerprint != key.expected_consumer_fingerprint
        {
            return Ok(ConsumerAttachmentStatus::IncarnationMismatch);
        }
        let agent_mode = metadata.initial_worker_metadata.agent_mode;
        if agent_mode != AgentMode::Durable {
            return Ok(ConsumerAttachmentStatus::IncarnationMismatch);
        }
        let current = self
            .oplog_service
            .get_last_index(&consumer, agent_mode)
            .await;
        if !current.is_defined() {
            return Ok(ConsumerAttachmentStatus::Missing);
        }
        let mut topology = ConsumerAttachmentStatus::Missing;
        let mut deleting = false;
        for (_, entry) in self
            .oplog_service
            .read_exact(&consumer, agent_mode, OplogIndex::INITIAL, current.as_u64())
            .await
        {
            let OplogEntry::StreamSession { record, .. } = entry else {
                continue;
            };
            let record = self
                .oplog_service
                .download_payload(&consumer, agent_mode, record)
                .await
                .map_err(DurableStreamProducerError::Oplog)?;
            if !record.has_supported_format() {
                return Err(DurableStreamProducerError::CorruptHistory(
                    "unsupported or malformed durable Stream Session record".to_string(),
                ));
            }
            match record {
                StreamSessionRecordV1::ConsumerDeleting(record)
                    if record.consumer_environment_id == key.consumer_environment_id
                        && record.consumer == key.consumer
                        && record.consumer_fingerprint == key.expected_consumer_fingerprint =>
                {
                    deleting = true;
                }
                StreamSessionRecordV1::TopologyPrepared(record)
                    if record.session_key == key.session_key
                        && record.attachment.attachment_id == key.attachment_id
                        && record.attachment.stream_id == key.stream_id =>
                {
                    if record.attachment.epoch < key.epoch {
                        continue;
                    }
                    if record.attachment.epoch > key.epoch {
                        return Ok(ConsumerAttachmentStatus::EpochMismatch);
                    }
                    if record.attachment != *key {
                        return Ok(ConsumerAttachmentStatus::IncarnationMismatch);
                    }
                    if expected_mapping.is_some_and(|mapping| mapping != &record.mapping) {
                        continue;
                    }
                    if topology == ConsumerAttachmentStatus::Missing {
                        topology = ConsumerAttachmentStatus::Prepared;
                    }
                }
                StreamSessionRecordV1::TopologyActivated(record)
                    if record.session_key == key.session_key
                        && record.attachment.attachment_id == key.attachment_id
                        && record.attachment.stream_id == key.stream_id =>
                {
                    if record.attachment.epoch < key.epoch {
                        continue;
                    }
                    if record.attachment.epoch > key.epoch {
                        return Ok(ConsumerAttachmentStatus::EpochMismatch);
                    }
                    if record.attachment != *key {
                        return Ok(ConsumerAttachmentStatus::IncarnationMismatch);
                    }
                    if expected_mapping.is_some_and(|mapping| mapping != &record.mapping) {
                        continue;
                    }
                    if expected_mapping.is_none() && topology == ConsumerAttachmentStatus::Active {
                        continue;
                    }
                    if topology != ConsumerAttachmentStatus::Prepared {
                        return Err(DurableStreamProducerError::CorruptHistory(
                            "durable topology activation has no matching preparation".to_string(),
                        ));
                    }
                    topology = ConsumerAttachmentStatus::Active;
                }
                _ => {}
            }
        }
        if deleting {
            return Ok(ConsumerAttachmentStatus::Deleting);
        }
        match (attachment_authority, topology) {
            (Some(_), topology) => Ok(topology),
            (None, ConsumerAttachmentStatus::Prepared) => Ok(ConsumerAttachmentStatus::Prepared),
            (None, ConsumerAttachmentStatus::Active) => {
                Err(DurableStreamProducerError::CorruptHistory(
                    "durable topology activation precedes session attachment".to_string(),
                ))
            }
            (None, topology) => Ok(topology),
        }
    }

    async fn journal_inspection(
        &self,
        key: &StreamAttachmentKeyV1,
    ) -> Result<Option<ConsumerJournalInspection>, DurableStreamProducerError> {
        let consumer = OwnedAgentId::new(key.consumer_environment_id, &key.consumer);
        let Some(metadata) = self.worker_service.get(&consumer).await else {
            return Ok(None);
        };
        if metadata.initial_worker_metadata.fingerprint != key.expected_consumer_fingerprint
            || metadata.initial_worker_metadata.agent_mode != AgentMode::Durable
        {
            return Ok(None);
        }
        let current = self
            .oplog_service
            .get_last_index(&consumer, AgentMode::Durable)
            .await;
        if !current.is_defined() {
            return Ok(None);
        }
        let mut offsets = Vec::new();
        let mut overlay = None;
        for (_, entry) in self
            .oplog_service
            .read_exact(
                &consumer,
                AgentMode::Durable,
                OplogIndex::INITIAL,
                current.as_u64(),
            )
            .await
        {
            let OplogEntry::StreamSession { record, .. } = entry else {
                continue;
            };
            let record = self
                .oplog_service
                .download_payload(&consumer, AgentMode::Durable, record)
                .await
                .map_err(DurableStreamProducerError::Oplog)?;
            match record {
                StreamSessionRecordV1::ConsumerItemValue(record)
                    if record.session_key == key.session_key
                        && record.stream_id == key.stream_id =>
                {
                    if record.consumer_read_ordinal != offsets.len() as u64 {
                        return Err(DurableStreamProducerError::CorruptHistory(
                            "consumer value journal contains a read-ordinal gap".to_string(),
                        ));
                    }
                    for index in 0..record.logical_item_count() {
                        offsets.push(record.source_offset_at(index).ok_or_else(|| {
                            DurableStreamProducerError::CorruptHistory(
                                "packed-u8 consumer journal offset range is invalid".to_string(),
                            )
                        })?);
                    }
                }
                StreamSessionRecordV1::ConsumerTerminal(record)
                    if record.session_key == key.session_key
                        && record.stream_id == key.stream_id =>
                {
                    if record.consumer_read_ordinal != offsets.len() as u64 {
                        return Err(DurableStreamProducerError::CorruptHistory(
                            "consumer terminal journal contains a read-ordinal gap".to_string(),
                        ));
                    }
                    offsets.push(record.source_offset);
                }
                StreamSessionRecordV1::SourceUnavailable(record)
                    if record.key.session_key == key.session_key
                        && record.key.stream_id == key.stream_id =>
                {
                    if record.consumer_read_ordinal != offsets.len() as u64 {
                        return Err(DurableStreamProducerError::CorruptHistory(
                            "source-unavailable overlay contains a read-ordinal gap".to_string(),
                        ));
                    }
                    match overlay {
                        Some(existing) if existing != record.source_offset => {
                            return Err(DurableStreamProducerError::CorruptHistory(
                                "conflicting source-unavailable overlays".to_string(),
                            ));
                        }
                        Some(_) => {}
                        None => overlay = Some(record.source_offset),
                    }
                }
                _ => {}
            }
        }
        Ok(Some(ConsumerJournalInspection {
            source_offsets: offsets,
            source_unavailable: overlay,
        }))
    }

    async fn commit_source_unavailable(
        &self,
        key: &StreamAttachmentKeyV1,
        source_offset: StreamOffsetV1,
        consumer_read_ordinal: u64,
    ) -> Result<(), DurableStreamProducerError> {
        let rpc = self.rpc.as_ref().ok_or_else(|| {
            DurableStreamProducerError::Oplog(
                "consumer probe has no route for a source-unavailable overlay".to_string(),
            )
        })?;
        rpc.control_durable_stream_attachment(
            StreamAttachmentControlRequestV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                mapping: None,
                operation: StreamAttachmentControlOperationV1::SourceUnavailable {
                    key: key.clone(),
                    source_offset,
                    consumer_read_ordinal,
                },
            },
            &AuthCtx::System,
        )
        .await
        .map_err(|error| DurableStreamProducerError::Oplog(error.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl StreamAttachmentControl for DurableStreamProducer {
    #[tracing::instrument(
        name = "durable_stream.attachment.prepare",
        skip_all,
        fields(attachment_id = %key.attachment_id.0, stream_id = %key.stream_id, epoch = key.epoch)
    )]
    async fn prepare_attachment(
        &self,
        key: StreamAttachmentKeyV1,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcomeV1<StreamAttachmentViewV1>, DurableStreamProducerError> {
        let lease_expires_at_millis = attachment_lease_expiry(now_millis)?;
        let outcome = self
            .persist_attachment_record(StreamSessionRecordV1::AttachmentPrepared(
                StreamAttachmentPreparedRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    key: key.clone(),
                    prepared_at_millis: now_millis,
                    lease_expires_at_millis,
                },
            ))
            .await?;
        let replayed = outcome == AttachmentApplyOutcome::Replayed;
        crate::metrics::durable_stream::record_attachment_operation(
            "prepare",
            if replayed { "replayed" } else { "committed" },
        );
        crate::metrics::durable_stream::record_lease_remaining(
            lease_expires_at_millis.saturating_sub(now_millis),
        );
        Ok(ProducerWriteOutcomeV1 {
            value: self.attachment_view(&key).await?,
            replayed,
        })
    }

    #[tracing::instrument(
        name = "durable_stream.attachment.activate",
        skip_all,
        fields(attachment_id = %key.attachment_id.0, stream_id = %key.stream_id, epoch = key.epoch)
    )]
    async fn activate_attachment(
        &self,
        key: StreamAttachmentKeyV1,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcomeV1<StreamAttachmentViewV1>, DurableStreamProducerError> {
        let lease_expires_at_millis = attachment_lease_expiry(now_millis)?;
        let outcome = self
            .persist_attachment_record(StreamSessionRecordV1::AttachmentActivated(
                StreamAttachmentActivatedRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    key: key.clone(),
                    activated_at_millis: now_millis,
                    lease_expires_at_millis,
                },
            ))
            .await?;
        let replayed = outcome == AttachmentApplyOutcome::Replayed;
        crate::metrics::durable_stream::record_attachment_operation(
            "activate",
            if replayed { "replayed" } else { "committed" },
        );
        crate::metrics::durable_stream::record_lease_remaining(
            lease_expires_at_millis.saturating_sub(now_millis),
        );
        Ok(ProducerWriteOutcomeV1 {
            value: self.attachment_view(&key).await?,
            replayed,
        })
    }

    async fn detach_attachment(
        &self,
        key: &StreamAttachmentKeyV1,
    ) -> Result<StreamAttachmentViewV1, DurableStreamProducerError> {
        let view = self.attachment_view(key).await?;
        if !matches!(view.state, StreamAttachmentStateV1::Active) {
            return Err(DurableStreamProducerError::InvalidAttachmentState);
        }
        Ok(view)
    }

    #[tracing::instrument(
        name = "durable_stream.attachment.renew",
        skip_all,
        fields(attachment_id = %key.attachment_id.0, stream_id = %key.stream_id, epoch = key.epoch)
    )]
    async fn renew_attachment(
        &self,
        key: StreamAttachmentKeyV1,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcomeV1<StreamAttachmentViewV1>, DurableStreamProducerError> {
        let lease_expires_at_millis = attachment_lease_expiry(now_millis)?;
        let outcome = self
            .persist_attachment_record(StreamSessionRecordV1::AttachmentRenewed(
                StreamAttachmentRenewedRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    key: key.clone(),
                    renewed_at_millis: now_millis,
                    lease_expires_at_millis,
                },
            ))
            .await?;
        let replayed = outcome == AttachmentApplyOutcome::Replayed;
        crate::metrics::durable_stream::record_attachment_operation(
            "renew",
            if replayed { "replayed" } else { "committed" },
        );
        crate::metrics::durable_stream::record_lease_remaining(
            lease_expires_at_millis.saturating_sub(now_millis),
        );
        Ok(ProducerWriteOutcomeV1 {
            value: self.attachment_view(&key).await?,
            replayed,
        })
    }

    #[tracing::instrument(
        name = "durable_stream.attachment.finalize",
        skip_all,
        fields(attachment_id = %key.attachment_id.0, stream_id = %key.stream_id, epoch = key.epoch, reason = ?reason)
    )]
    async fn finalize_attachment(
        &self,
        key: StreamAttachmentKeyV1,
        reason: StreamAttachmentFinalizationReasonV1,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcomeV1<StreamAttachmentViewV1>, DurableStreamProducerError> {
        let outcome = self
            .persist_attachment_record(StreamSessionRecordV1::AttachmentFinalized(
                StreamAttachmentFinalizedRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    key: key.clone(),
                    finalized_at_millis: now_millis,
                    reason,
                },
            ))
            .await?;
        let replayed = outcome == AttachmentApplyOutcome::Replayed;
        crate::metrics::durable_stream::record_attachment_operation(
            "finalize",
            if replayed { "replayed" } else { "committed" },
        );
        Ok(ProducerWriteOutcomeV1 {
            value: self.attachment_view(&key).await?,
            replayed,
        })
    }

    async fn inspect_attachments(&self) -> Vec<StreamAttachmentViewV1> {
        self.index.lock().await.attachment_views()
    }
}

impl DurableStreamProducer {
    pub(crate) async fn deletion_started(&self) -> bool {
        let index = self.index.lock().await;
        index.deleting || index.consumer_deleting
    }

    pub(crate) async fn deletion_diagnostics(&self) -> StreamDeletionDiagnosticsV1 {
        let index = self.index.lock().await;
        let mut cascade_completed = index
            .cascade_outbox
            .iter()
            .map(|(key, result)| (key.clone(), result.clone()))
            .collect::<Vec<_>>();
        cascade_completed.sort_by_key(|(key, _)| (key.stream_id, key.attachment_id, key.epoch));
        StreamDeletionDiagnosticsV1 {
            deleting: index.deleting,
            attachments: index.attachment_views(),
            cascade_completed,
        }
    }

    pub(crate) async fn cascade_deletion(
        &self,
        now_millis: u64,
        probe: &(dyn StreamAttachmentConsumerProbe + Send + Sync),
    ) -> Result<(), DurableStreamProducerError> {
        self.commit_deletion_barrier(now_millis, false).await?;
        let dependents = self.index.lock().await.incomplete_cascade_dependents();
        for key in dependents {
            let status = probe.status(&key).await?;
            let result = match status {
                ConsumerAttachmentStatus::Deleting | ConsumerAttachmentStatus::Missing => {
                    StreamCascadeDependentResultV1::ConsumerDeleted
                }
                ConsumerAttachmentStatus::IncarnationMismatch
                | ConsumerAttachmentStatus::EpochMismatch => {
                    StreamCascadeDependentResultV1::ConsumerIncarnationChanged
                }
                ConsumerAttachmentStatus::Prepared | ConsumerAttachmentStatus::Active => {
                    let inspection = probe
                        .journal_inspection(&key)
                        .await?
                        .ok_or(DurableStreamProducerError::InvalidAttachmentState)?;
                    let producer_offsets = {
                        let index = self.index.lock().await;
                        index
                            .streams
                            .get(&key.stream_id)
                            .ok_or(DurableStreamProducerError::UnknownStream(key.stream_id))?
                            .events
                            .values()
                            .map(|event| event.offset)
                            .collect::<Vec<_>>()
                    };
                    if inspection.source_offsets.len() > producer_offsets.len()
                        || producer_offsets[..inspection.source_offsets.len()]
                            != inspection.source_offsets
                    {
                        return Err(DurableStreamProducerError::CorruptHistory(
                            "consumer journal is not an exact prefix of producer history"
                                .to_string(),
                        ));
                    }
                    if inspection.source_offsets.len() == producer_offsets.len() {
                        StreamCascadeDependentResultV1::ConsumerJournalComplete
                    } else {
                        let first_unjournaled_offset =
                            producer_offsets[inspection.source_offsets.len()];
                        if let Some(existing) = inspection.source_unavailable {
                            if existing != first_unjournaled_offset {
                                return Err(DurableStreamProducerError::CorruptHistory(
                                    "source-unavailable overlay does not identify the first unjournaled producer position"
                                        .to_string(),
                                ));
                            }
                        } else {
                            probe
                                .commit_source_unavailable(
                                    &key,
                                    first_unjournaled_offset,
                                    inspection.source_offsets.len() as u64,
                                )
                                .await?;
                        }
                        StreamCascadeDependentResultV1::SourceUnavailable {
                            first_unjournaled_offset,
                        }
                    }
                }
            };
            self.commit_cascade_outbox(key, now_millis, result).await?;
        }
        let incomplete = self.index.lock().await.incomplete_cascade_dependents();
        if incomplete.is_empty() {
            Ok(())
        } else {
            Err(DurableStreamProducerError::DeletionBlocked(incomplete))
        }
    }

    async fn commit_deletion_barrier(
        &self,
        now_millis: u64,
        require_no_dependents: bool,
    ) -> Result<(), DurableStreamProducerError> {
        let mut index = self.index.lock().await;
        if index.deleting {
            crate::metrics::durable_stream::record_producer_operation("deletion_barrier", true);
            return Ok(());
        }
        if require_no_dependents {
            let dependents = index.live_dependents();
            if !dependents.is_empty() {
                return Err(DurableStreamProducerError::DeletionBlocked(dependents));
            }
        }
        let mut open_streams = index
            .streams
            .iter()
            .filter_map(|(stream_id, stream)| {
                (!stream.terminal).then_some((*stream_id, stream.next_sequence))
            })
            .collect::<Vec<_>>();
        open_streams.sort_by_key(|(stream_id, _)| *stream_id);
        let environment_id = self.environment_id;
        let producer = self.producer.clone();
        let producer_fingerprint = self.producer_fingerprint;
        let entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |first_index| {
                let mut records = Vec::with_capacity(open_streams.len() + 1);
                for (position, (stream_id, sequence)) in open_streams.into_iter().enumerate() {
                    let oplog_index = OplogIndex::from_u64(first_index.as_u64() + position as u64);
                    records.push(DurableStreamOplogRecord::Cancel(StreamCancelRecordV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        stream_id,
                        producer_fingerprint,
                        sequence,
                        offset: StreamOffsetV1::new(oplog_index, 0),
                        authored_by: StreamTerminalAuthorV1::Protocol,
                        role: StreamCancelRoleV1::System,
                        reason: StreamCancelReasonV1::ProducerDeleting,
                        details: None,
                    }));
                }
                records.push(DurableStreamOplogRecord::Session(Box::new(
                    StreamSessionRecordV1::ProducerDeleting(StreamProducerDeletingRecordV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        producer_environment_id: environment_id,
                        producer,
                        producer_fingerprint,
                        deleting_at_millis: now_millis,
                    }),
                )));
                records
            }))
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        self.commit().await;
        let mut terminal_events = Vec::new();
        for (oplog_index, entry) in entries {
            match entry {
                OplogEntry::StreamCancel { record, .. } => {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(DurableStreamProducerError::Oplog)?;
                    terminal_events.push(index.apply_cancel(
                        oplog_index,
                        record,
                        self.producer_fingerprint,
                    )?);
                }
                OplogEntry::StreamSession { record, .. } => {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(DurableStreamProducerError::Oplog)?;
                    index.apply_deletion_record(
                        &record,
                        self.environment_id,
                        &self.producer,
                        self.producer_fingerprint,
                    )?;
                }
                _ => {
                    return Err(DurableStreamProducerError::CorruptHistory(
                        "durable deletion barrier batch contains an unexpected entry".to_string(),
                    ));
                }
            }
        }
        drop(index);
        let terminal_count = terminal_events.len();
        for event in terminal_events {
            self.cancel_source(event.stream_id);
            self.bus(event.stream_id)?
                .publish_committed(DurableLiveStreamEvent {
                    offset: event.offset,
                    payload: event,
                })
                .await?;
        }
        self.record_terminal_streams(terminal_count);
        crate::metrics::durable_stream::record_producer_operation("deletion_barrier", false);
        tracing::debug!(
            terminal_streams = terminal_count,
            "Durable stream producer deletion barrier committed"
        );
        self.notify_session_records_changed();
        Ok(())
    }

    async fn commit_cascade_outbox(
        &self,
        key: StreamAttachmentKeyV1,
        now_millis: u64,
        result: StreamCascadeDependentResultV1,
    ) -> Result<(), DurableStreamProducerError> {
        let mut index = self.index.lock().await;
        if let Some(existing) = index.cascade_outbox.get(&key) {
            return if existing == &result {
                crate::metrics::durable_stream::record_cascade("replayed");
                Ok(())
            } else {
                Err(DurableStreamProducerError::CorruptHistory(
                    "conflicting durable cascade completion".to_string(),
                ))
            };
        }
        let outcome = match &result {
            StreamCascadeDependentResultV1::ConsumerDeleted => "consumer_deleted",
            StreamCascadeDependentResultV1::ConsumerIncarnationChanged => {
                "consumer_incarnation_changed"
            }
            StreamCascadeDependentResultV1::ConsumerJournalComplete => "journal_complete",
            StreamCascadeDependentResultV1::SourceUnavailable { .. } => "source_unavailable",
        };
        let attachment_id = key.attachment_id;
        let stream_id = key.stream_id;
        let epoch = key.epoch;
        let record = StreamSessionRecordV1::CascadeOutbox(StreamCascadeOutboxRecordV1 {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            key,
            completed_at_millis: now_millis,
            result,
        });
        self.oplog
            .add(OplogEntry::stream_session(OplogPayload::Inline(Box::new(
                record.clone(),
            ))))
            .await;
        self.commit().await;
        index.apply_deletion_record(
            &record,
            self.environment_id,
            &self.producer,
            self.producer_fingerprint,
        )?;
        crate::metrics::durable_stream::record_cascade(outcome);
        tracing::debug!(
            attachment_id = %attachment_id.0,
            stream_id = %stream_id,
            epoch,
            outcome,
            "Durable stream cascade outbox committed"
        );
        Ok(())
    }

    async fn attachment_view(
        &self,
        key: &StreamAttachmentKeyV1,
    ) -> Result<StreamAttachmentViewV1, DurableStreamProducerError> {
        self.index
            .lock()
            .await
            .attachment_views()
            .into_iter()
            .find(|view| view.key == *key)
            .ok_or(DurableStreamProducerError::InvalidAttachmentState)
    }

    pub(crate) async fn reconcile_attachments_configured(
        &self,
        now_millis: u64,
        renewal_target_millis: u64,
        batch_size: usize,
        probe: &(dyn StreamAttachmentConsumerProbe + Send + Sync),
    ) -> Result<usize, DurableStreamProducerError> {
        let (deleting, candidates) = {
            let index = self.index.lock().await;
            let deleting = index.deleting;
            let mut candidates = index
                .attachments
                .values()
                .filter(|attachment| {
                    !matches!(
                        attachment.state,
                        IndexedStreamAttachmentState::Finalized { .. }
                    )
                })
                .map(|attachment| {
                    let stream = index
                        .streams
                        .get(&attachment.key.stream_id)
                        .expect("durable attachment index points at a missing producer stream");
                    (
                        attachment.clone(),
                        stream.terminal,
                        stream
                            .events
                            .values()
                            .map(|event| event.offset)
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>();
            candidates.sort_by_key(|(attachment, _, _)| {
                (
                    attachment.key.stream_id,
                    attachment.key.attachment_id,
                    attachment.key.epoch,
                )
            });
            if !candidates.is_empty() {
                let start = self
                    .reconciliation_cursor
                    .fetch_add(batch_size, Ordering::Relaxed)
                    % candidates.len();
                candidates.rotate_left(start);
            }
            candidates.truncate(batch_size);
            (deleting, candidates)
        };
        let mut changed = 0;
        let mut first_error = None;
        for (attachment, producer_terminal, producer_offsets) in candidates {
            match &attachment.state {
                IndexedStreamAttachmentState::Prepared {
                    lease_expires_at_millis,
                    ..
                }
                | IndexedStreamAttachmentState::Active {
                    lease_expires_at_millis,
                    ..
                } => crate::metrics::durable_stream::record_lease_remaining(
                    lease_expires_at_millis.saturating_sub(now_millis),
                ),
                IndexedStreamAttachmentState::Finalized { .. } => {}
            }
            let status = match probe.status(&attachment.key).await {
                Ok(status) => status,
                Err(error) => {
                    crate::metrics::durable_stream::record_reconciliation("probe_error");
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                    continue;
                }
            };
            let journal_complete = if producer_terminal {
                match probe.journal_inspection(&attachment.key).await {
                    Ok(Some(inspection)) => {
                        inspection.source_unavailable.is_none()
                            && inspection.source_offsets == producer_offsets
                    }
                    Ok(None) => false,
                    Err(error) => {
                        crate::metrics::durable_stream::record_reconciliation("journal_error");
                        if first_error.is_none() {
                            first_error = Some(error);
                        }
                        continue;
                    }
                }
            } else {
                false
            };
            let action = if journal_complete {
                Some(ReconciliationAction::Finalize(
                    StreamAttachmentFinalizationReasonV1::ConsumerFinalized,
                ))
            } else {
                match (attachment.state, status) {
                (
                    IndexedStreamAttachmentState::Prepared { .. },
                    ConsumerAttachmentStatus::Active,
                ) => Some(ReconciliationAction::Activate),
                (
                    IndexedStreamAttachmentState::Prepared {
                        prepared_at_millis,
                        ..
                    },
                    ConsumerAttachmentStatus::Missing,
                ) if now_millis.saturating_sub(prepared_at_millis)
                    >= golem_common::base_model::durable_stream::STREAM_ATTACHMENT_ABANDONED_PREPARE_MILLIS =>
                {
                    Some(ReconciliationAction::Finalize(
                        StreamAttachmentFinalizationReasonV1::PrepareAbandoned,
                    ))
                }
                (
                    IndexedStreamAttachmentState::Active {
                        activated_at_millis,
                        ..
                    },
                    ConsumerAttachmentStatus::Active,
                ) if now_millis.saturating_sub(activated_at_millis)
                    >= renewal_target_millis =>
                {
                    Some(ReconciliationAction::Renew)
                }
                (_, ConsumerAttachmentStatus::Deleting) => {
                    Some(ReconciliationAction::Finalize(
                        StreamAttachmentFinalizationReasonV1::ConsumerDeleted,
                    ))
                }
                (_, ConsumerAttachmentStatus::IncarnationMismatch) => {
                    Some(ReconciliationAction::Finalize(
                        StreamAttachmentFinalizationReasonV1::ConsumerIncarnationChanged,
                    ))
                }
                (_, ConsumerAttachmentStatus::EpochMismatch) => Some(
                    ReconciliationAction::Finalize(
                        StreamAttachmentFinalizationReasonV1::Reconciled,
                    ),
                ),
                (IndexedStreamAttachmentState::Active { .. }, ConsumerAttachmentStatus::Missing) => {
                    Some(ReconciliationAction::Finalize(
                        StreamAttachmentFinalizationReasonV1::ConsumerDeleted,
                    ))
                }
                _ => None,
                }
            };
            let action = match (deleting, action) {
                (true, Some(ReconciliationAction::Activate | ReconciliationAction::Renew)) => None,
                (_, action) => action,
            };
            let action_outcome = match &action {
                Some(ReconciliationAction::Activate) => "activated",
                Some(ReconciliationAction::Renew) => "renewed",
                Some(ReconciliationAction::Finalize(_)) => "finalized",
                None => "unchanged",
            };
            let replayed = match action {
                Some(ReconciliationAction::Activate) => self
                    .activate_attachment(attachment.key, now_millis)
                    .await
                    .map(|outcome| outcome.replayed),
                Some(ReconciliationAction::Renew) => self
                    .renew_attachment(attachment.key, now_millis)
                    .await
                    .map(|outcome| outcome.replayed),
                Some(ReconciliationAction::Finalize(reason)) => self
                    .finalize_attachment(attachment.key, reason, now_millis)
                    .await
                    .map(|outcome| outcome.replayed),
                None => Ok(true),
            };
            let replayed = match replayed {
                Ok(replayed) => replayed,
                Err(error) => {
                    crate::metrics::durable_stream::record_reconciliation("write_error");
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                    continue;
                }
            };
            crate::metrics::durable_stream::record_reconciliation(if replayed {
                "replayed"
            } else {
                action_outcome
            });
            if !replayed {
                changed += 1;
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(changed),
        }
    }
}

enum ReconciliationAction {
    Activate,
    Renew,
    Finalize(StreamAttachmentFinalizationReasonV1),
}

fn attachment_lease_expiry(now_millis: u64) -> Result<u64, DurableStreamProducerError> {
    now_millis
        .checked_add(STREAM_ATTACHMENT_LEASE_TTL_MILLIS)
        .ok_or(DurableStreamProducerError::CounterOverflow)
}

#[async_trait]
impl StreamSegmentSource for DurableStreamProducer {
    async fn read_segment(
        &self,
        handle: &DurableStreamHandleV1,
        after: Option<StreamOffsetV1>,
        through: Option<StreamOffsetV1>,
    ) -> Result<Vec<CommittedProducerStreamEventV1>, DurableStreamProducerError> {
        let index = self.index.lock().await;
        read_segment_from_index(&index, handle, after, through)
    }
}

#[async_trait]
impl AttachedStreamSegmentSource for DurableStreamProducer {
    async fn read_attached_segment(
        &self,
        attachment: &StreamAttachmentKeyV1,
        handle: &DurableStreamHandleV1,
        now_millis: u64,
        after: Option<StreamOffsetV1>,
        through: Option<StreamOffsetV1>,
    ) -> Result<Vec<CommittedProducerStreamEventV1>, DurableStreamProducerError> {
        if attachment.stream_id != handle.stream_id {
            return Err(DurableStreamProducerError::InvalidHandle);
        }
        let index = self.index.lock().await;
        index.validate_attachment_key(
            attachment,
            self.environment_id,
            &self.producer,
            self.producer_fingerprint,
        )?;
        let indexed_attachment = index
            .attachments
            .get(&(attachment.attachment_id, attachment.stream_id))
            .ok_or(DurableStreamProducerError::InvalidAttachmentState)?;
        validate_attachment_epoch(indexed_attachment, attachment)?;
        if indexed_attachment.key != *attachment {
            return Err(DurableStreamProducerError::AttachmentConflict);
        }
        match indexed_attachment.state {
            IndexedStreamAttachmentState::Active {
                lease_expires_at_millis,
                ..
            } if now_millis < lease_expires_at_millis => {}
            IndexedStreamAttachmentState::Active { .. } => {
                return Err(DurableStreamProducerError::LeaseExpired);
            }
            _ => return Err(DurableStreamProducerError::InvalidAttachmentState),
        }
        read_segment_from_index(&index, handle, after, through)
    }

    async fn wait_for_attached_segment(
        &self,
        attachment: &StreamAttachmentKeyV1,
        handle: &DurableStreamHandleV1,
        now_millis: u64,
        after: Option<StreamOffsetV1>,
    ) -> Result<Vec<CommittedProducerStreamEventV1>, DurableStreamProducerError> {
        let events = self
            .read_attached_segment(attachment, handle, now_millis, after, None)
            .await?;
        if !events.is_empty() {
            return Ok(events);
        }
        let mut reader = self.catch_up(handle.clone(), after).await?;
        match tokio::time::timeout(std::time::Duration::from_secs(1), reader.next()).await {
            Ok(event) => Ok(event?.into_iter().collect()),
            Err(_) => Ok(Vec::new()),
        }
    }
}

fn read_segment_from_index(
    index: &ProducerStreamIndex,
    handle: &DurableStreamHandleV1,
    after: Option<StreamOffsetV1>,
    through: Option<StreamOffsetV1>,
) -> Result<Vec<CommittedProducerStreamEventV1>, DurableStreamProducerError> {
    validate_version(handle.format_version)?;
    if !index
        .registrations
        .get(&handle.stream_id)
        .is_some_and(|record| &record.handle == handle)
    {
        return Err(DurableStreamProducerError::InvalidHandle);
    }
    for offset in [after, through].into_iter().flatten() {
        StreamOffsetV1::from_bytes(*offset.as_bytes())
            .map_err(|error| DurableStreamProducerError::InvalidOffset(error.to_string()))?;
    }
    let stream = index
        .streams
        .get(&handle.stream_id)
        .ok_or(DurableStreamProducerError::UnknownStream(handle.stream_id))?;
    if after.is_some_and(|cursor| !stream.events.values().any(|event| event.offset == cursor))
        || through.is_some_and(|cursor| !stream.events.values().any(|event| event.offset == cursor))
    {
        return Err(DurableStreamProducerError::CursorUnavailable);
    }
    if after
        .zip(through)
        .is_some_and(|(after, through)| after > through)
    {
        return Err(DurableStreamProducerError::InvalidOffset(
            "catch-up end precedes its cursor".to_string(),
        ));
    }
    Ok(stream
        .events
        .values()
        .filter(|event| after.is_none_or(|after| event.offset > after))
        .filter(|event| through.is_none_or(|through| event.offset <= through))
        .cloned()
        .collect())
}

pub(crate) struct DurableCatchUpReader {
    bus: Arc<DurableLiveStreamBus<CommittedProducerStreamEventV1>>,
    subscription: Option<DurableLiveStreamSubscription<CommittedProducerStreamEventV1>>,
    history: VecDeque<CommittedProducerStreamEventV1>,
    join_high_water: Option<StreamOffsetV1>,
    last_delivered: Option<StreamOffsetV1>,
    terminal_delivered: bool,
}

impl DurableCatchUpReader {
    pub(crate) async fn next(
        &mut self,
    ) -> Result<Option<CommittedProducerStreamEventV1>, DurableStreamProducerError> {
        if self.terminal_delivered {
            return Ok(None);
        }
        if let Some(event) = self.history.pop_front() {
            let event = self.deliver(event)?;
            self.release_subscription_if_complete();
            return Ok(Some(event));
        }
        loop {
            let Some(subscription) = self.subscription.as_mut() else {
                return Ok(None);
            };
            let Some(event) = subscription.recv().await else {
                self.release_subscription().await;
                return Ok(None);
            };
            if self
                .join_high_water
                .is_some_and(|high_water| event.offset <= high_water)
                || self
                    .last_delivered
                    .is_some_and(|last_delivered| event.offset <= last_delivered)
            {
                continue;
            }
            let event = self.deliver(event.payload)?;
            self.release_subscription_if_complete();
            return Ok(Some(event));
        }
    }

    fn release_subscription_if_complete(&mut self) {
        if self.terminal_delivered {
            self.release_subscription_in_background();
        }
    }

    fn release_subscription_in_background(&mut self) {
        let Some(subscription) = self.subscription.take() else {
            return;
        };
        let reader_id = subscription.reader_id();
        drop(subscription);
        let bus = self.bus.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                bus.unsubscribe(reader_id).await;
            });
        }
    }

    async fn release_subscription(&mut self) {
        if let Some(subscription) = self.subscription.take() {
            self.bus.unsubscribe(subscription.reader_id()).await;
        }
    }

    fn deliver(
        &mut self,
        event: CommittedProducerStreamEventV1,
    ) -> Result<CommittedProducerStreamEventV1, DurableStreamProducerError> {
        if self
            .last_delivered
            .is_some_and(|last_delivered| event.offset <= last_delivered)
        {
            return Err(DurableStreamProducerError::CorruptHistory(
                "catch-up reader observed a non-increasing offset".to_string(),
            ));
        }
        self.last_delivered = Some(event.offset);
        self.terminal_delivered = event.is_terminal();
        Ok(event)
    }
}

impl Drop for DurableCatchUpReader {
    fn drop(&mut self) {
        self.release_subscription_in_background();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{
        AgentError, AttachedStreamSegmentSource, CommittedProducerStreamEventPayloadV1,
        CommittedProducerStreamEventV1, ConsumerAttachmentStatus, ConsumerJournalInspection,
        DurableCatchUpReader, DurableLiveStreamBus, DurableLiveStreamBusError, DurableStreamCommit,
        DurableStreamProducer, DurableStreamProducerError, ProducerRegistrationRequestV1,
        ProducerStreamIndex, StreamAttachmentConsumerProbe, StreamAttachmentControl,
        StreamAttachmentStateV1, StreamSegmentSource, registration_record,
    };
    use crate::services::oplog::{
        CommitLevel, DurableStreamOplogRecord, Oplog, OplogAddReceipt, OplogReadSource,
        OrderedOplogStart, PendingUpload, checked_range_end, exact_from_source, fail_stop,
    };
    use async_trait::async_trait;
    use futures::FutureExt;
    use golem_common::base_model::component::{ComponentId, ComponentRevision};
    use golem_common::base_model::durable_stream::{
        AttachmentId, AttemptId, DURABLE_STREAM_FORMAT_VERSION, InputStreamHighWaterV1,
        MAX_DURABLE_STREAM_ITEM_SIZE, MAX_DURABLE_STREAMS_PER_SESSION, MAX_LIVE_JOIN_BUFFER_SIZE,
        MAX_NEW_STREAM_HANDLES_PER_VALUE, MAX_PACKED_U8_STREAM_ITEM_SIZE,
        MAX_STREAM_VALUE_TRAVERSAL_DEPTH, PersistedStreamInvocationDescriptorV1,
        STREAM_ATTACHMENT_ABANDONED_PREPARE_MILLIS, STREAM_ATTACHMENT_LEASE_TTL_MILLIS,
        SessionStreamRoleV1, StartAttemptDescriptorV1, StreamAttachmentFinalizationReasonV1,
        StreamAttachmentKeyV1, StreamCancelReasonV1, StreamCancelRoleV1,
        StreamCascadeDependentResultV1, StreamConsumerDeletingRecordV1,
        StreamConsumerItemValueRecordV1, StreamEndResultV1, StreamId, StreamInvocationIdV1,
        StreamItemsPayloadV1, StreamItemsRecordV1, StreamOffsetV1, StreamRegistrationCoordinateV1,
        StreamRootKindV1, StreamSessionInvocationResultRecordV1, StreamSessionMappingRecordV1,
        StreamSessionMappingUpdateRecordV1, StreamSessionPreparedRecordV1, StreamSessionRecordV1,
        StreamSourceKindV1, StreamTerminalAuthorV1, StreamTopologyActivatedRecordV1,
        StreamTopologyPreparedRecordV1, StreamValuePathStepV1,
    };
    use golem_common::base_model::environment::EnvironmentId;
    use golem_common::base_model::{AgentFingerprint, AgentId, IdempotencyKey, OplogIndex};
    use golem_common::model::AgentInvocationPayload;
    use golem_common::model::invocation_context::TraceId;
    use golem_common::model::oplog::payload::OplogPayload;
    use golem_common::model::oplog::{OplogEntry, PayloadId, RawOplogPayload};
    use golem_schema::schema::SchemaFingerprintV1;
    use std::collections::{BTreeMap, VecDeque};
    use std::fmt::Debug;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use test_r::{test, timeout};
    use tokio::sync::{Barrier, oneshot};
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    #[derive(Default)]
    struct TestOplogState {
        entries: BTreeMap<OplogIndex, OplogEntry>,
        committed: OplogIndex,
        commit_count: u64,
    }

    #[derive(Default)]
    pub(crate) struct TestOplog {
        state: Mutex<TestOplogState>,
    }

    impl Debug for TestOplog {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.debug_struct("TestOplog").finish()
        }
    }

    impl TestOplog {
        fn committed_length(&self) -> u64 {
            self.state.lock().unwrap().committed.as_u64()
        }

        fn commit_count(&self) -> u64 {
            self.state.lock().unwrap().commit_count
        }

        fn entries(&self) -> Vec<OplogEntry> {
            self.state
                .lock()
                .unwrap()
                .entries
                .values()
                .cloned()
                .collect()
        }
    }

    #[async_trait]
    impl Oplog for TestOplog {
        async fn add(&self, entry: OplogEntry) -> OplogIndex {
            let mut state = self.state.lock().unwrap();
            let index = state
                .entries
                .last_key_value()
                .map_or(OplogIndex::INITIAL, |(index, _)| index.next());
            state.entries.insert(index, entry);
            index
        }

        fn enqueue_add(&self, entry: OplogEntry) -> OplogAddReceipt {
            let mut state = self.state.lock().unwrap();
            let index = state
                .entries
                .last_key_value()
                .map_or(OplogIndex::INITIAL, |(index, _)| index.next());
            state.entries.insert(index, entry);
            Box::pin(async move { index })
        }

        async fn drop_prefix(&self, last_dropped_id: OplogIndex) -> u64 {
            let mut state = self.state.lock().unwrap();
            let before = state.entries.len();
            state.entries.retain(|index, _| *index > last_dropped_id);
            (before - state.entries.len()) as u64
        }

        async fn commit(&self, _level: CommitLevel) -> BTreeMap<OplogIndex, OplogEntry> {
            let mut state = self.state.lock().unwrap();
            let committed = state
                .entries
                .iter()
                .filter(|(index, _)| **index > state.committed)
                .map(|(index, entry)| (*index, entry.clone()))
                .collect();
            state.committed = state
                .entries
                .last_key_value()
                .map_or(state.committed, |(index, _)| *index);
            state.commit_count += 1;
            committed
        }

        async fn current_oplog_index(&self) -> OplogIndex {
            self.state
                .lock()
                .unwrap()
                .entries
                .last_key_value()
                .map_or(OplogIndex::NONE, |(index, _)| *index)
        }

        async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
            self.state
                .lock()
                .unwrap()
                .entries
                .iter()
                .rev()
                .find_map(|(index, entry)| (!entry.is_hint()).then_some(*index))
        }

        async fn wait_for_replicas(&self, _replicas: u8, _timeout: Duration) -> bool {
            true
        }

        async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
            self.state
                .lock()
                .unwrap()
                .entries
                .get(&oplog_index)
                .cloned()
                .expect("missing test oplog entry")
        }

        async fn read_exact(
            &self,
            oplog_index: OplogIndex,
            n: u64,
        ) -> BTreeMap<OplogIndex, OplogEntry> {
            let state = self.state.lock().unwrap();
            let end = fail_stop(checked_range_end(oplog_index, n));
            let entries = end.map_or_else(BTreeMap::new, |end| {
                state
                    .entries
                    .range(oplog_index..=end)
                    .map(|(index, entry)| (*index, entry.clone()))
                    .collect()
            });
            fail_stop(exact_from_source(
                OplogReadSource::Other("durable stream test oplog"),
                oplog_index,
                n,
                entries,
            ))
        }

        async fn length(&self) -> u64 {
            self.state.lock().unwrap().entries.len() as u64
        }

        async fn upload_raw_payload(&self, data: Vec<u8>) -> Result<RawOplogPayload, String> {
            Ok(RawOplogPayload::SerializedInline(data))
        }

        async fn download_raw_payload(
            &self,
            _payload_id: PayloadId,
            _md5_hash: Vec<u8>,
        ) -> Result<Vec<u8>, String> {
            Err("test oplog has no external payloads".to_string())
        }

        async fn add_start_with_reserved_raw_payload(
            &self,
            serialized_request: Vec<u8>,
            build_start: Box<dyn FnOnce(RawOplogPayload) -> Result<OplogEntry, String> + Send>,
        ) -> Result<OrderedOplogStart, String> {
            let entry = build_start(RawOplogPayload::SerializedInline(serialized_request))?;
            let index = self.add(entry.clone()).await;
            Ok(OrderedOplogStart {
                index,
                entry,
                pending_upload: PendingUpload::already_durable(),
            })
        }

        async fn add_start_with_indexed_reserved_raw_payload(
            &self,
            build_request: crate::services::oplog::IndexedReservedStartBuilder,
        ) -> Result<OrderedOplogStart, String> {
            let mut state = self.state.lock().unwrap();
            let index = state
                .entries
                .last_key_value()
                .map_or(OplogIndex::INITIAL, |(index, _)| index.next());
            let (serialized_request, build_start) = build_request(index)?;
            let entry = build_start(RawOplogPayload::SerializedInline(serialized_request))?;
            state.entries.insert(index, entry.clone());
            Ok(OrderedOplogStart {
                index,
                entry,
                pending_upload: PendingUpload::already_durable(),
            })
        }

        async fn add_pair(
            &self,
            start: OplogEntry,
            make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
        ) -> (OplogIndex, OplogIndex) {
            let first = self.add(start).await;
            let second = self.add(make_second(first)).await;
            (first, second)
        }
    }

    #[test]
    async fn test_oplog_read_exact_includes_uncommitted_entries() {
        let oplog = TestOplog::default();
        let entry = OplogEntry::interrupted();
        let index = oplog.add(entry.clone()).await;

        let entries = oplog.read_exact(index, 1).await;

        assert_eq!(entries.get(&index), Some(&entry));
    }

    #[test]
    async fn test_oplog_read_exact_rejects_incomplete_range() {
        let oplog = TestOplog::default();
        let index = oplog.add(OplogEntry::interrupted()).await;

        let result = std::panic::AssertUnwindSafe(oplog.read_exact(index, 2))
            .catch_unwind()
            .await;

        assert!(
            result.is_err(),
            "read_exact accepted a range whose second entry is missing"
        );
    }

    #[test]
    async fn test_oplog_read_exact_accepts_single_entry_at_max_index() {
        let oplog = TestOplog::default();
        let index = OplogIndex::from_u64(u64::MAX);
        let entry = OplogEntry::interrupted();
        oplog
            .state
            .lock()
            .unwrap()
            .entries
            .insert(index, entry.clone());

        let entries = oplog.read_exact(index, 1).await;

        assert_eq!(entries.get(&index), Some(&entry));
    }

    pub(crate) struct TestIdentity {
        pub(crate) environment_id: EnvironmentId,
        pub(crate) agent_id: AgentId,
        pub(crate) fingerprint: AgentFingerprint,
        pub(crate) invocation: StreamInvocationIdV1,
    }

    pub(crate) fn identity() -> TestIdentity {
        let environment_id = EnvironmentId(Uuid::from_u128(1));
        let agent_id = AgentId {
            component_id: ComponentId(Uuid::from_u128(2)),
            agent_id: "producer".to_string(),
        };
        let fingerprint = AgentFingerprint(Uuid::from_u128(3));
        TestIdentity {
            environment_id,
            agent_id: agent_id.clone(),
            fingerprint,
            invocation: StreamInvocationIdV1 {
                callee_environment_id: environment_id,
                callee: agent_id,
                callee_fingerprint: fingerprint,
                idempotency_key: IdempotencyKey::new("invocation".to_string()),
            },
        }
    }

    pub(crate) fn registration(
        identity: &TestIdentity,
        coordinate: StreamRegistrationCoordinateV1,
        source_kind: StreamSourceKindV1,
    ) -> ProducerRegistrationRequestV1 {
        ProducerRegistrationRequestV1 {
            coordinate,
            source_invocation: identity.invocation.clone(),
            component_revision: ComponentRevision::INITIAL,
            element_schema_fingerprint: SchemaFingerprintV1([7; 32]),
            source_kind,
            session_mapping: None,
        }
    }

    fn root_registration(identity: &TestIdentity) -> ProducerRegistrationRequestV1 {
        registration(
            identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::InvocationOutput,
        )
    }

    fn attachment_key(
        identity: &TestIdentity,
        stream_id: golem_common::base_model::durable_stream::StreamId,
    ) -> StreamAttachmentKeyV1 {
        let consumer_environment_id = EnvironmentId(Uuid::from_u128(11));
        let consumer = AgentId {
            component_id: ComponentId(Uuid::from_u128(12)),
            agent_id: "consumer".to_string(),
        };
        let expected_consumer_fingerprint = AgentFingerprint(Uuid::from_u128(13));
        let consumer_invocation = StreamInvocationIdV1 {
            callee_environment_id: consumer_environment_id,
            callee: consumer.clone(),
            callee_fingerprint: expected_consumer_fingerprint,
            idempotency_key: IdempotencyKey::new("consumer-invocation".to_string()),
        };
        StreamAttachmentKeyV1 {
            attachment_id: AttachmentId::primary(
                consumer_environment_id,
                &consumer,
                &consumer_invocation.idempotency_key,
            )
            .unwrap(),
            stream_id,
            epoch: 1,
            session_key: consumer_invocation.clone(),
            producer_environment_id: identity.environment_id,
            producer: identity.agent_id.clone(),
            expected_producer_fingerprint: identity.fingerprint,
            consumer_environment_id,
            consumer,
            expected_consumer_fingerprint,
            consumer_invocation,
        }
    }

    struct FixedConsumerProbe(ConsumerAttachmentStatus);

    #[async_trait]
    impl StreamAttachmentConsumerProbe for FixedConsumerProbe {
        async fn status(
            &self,
            _key: &StreamAttachmentKeyV1,
        ) -> Result<ConsumerAttachmentStatus, DurableStreamProducerError> {
            Ok(self.0)
        }
    }

    struct CascadeConsumerProbe {
        status: ConsumerAttachmentStatus,
        inspection: std::sync::Mutex<ConsumerJournalInspection>,
        overlay_commits: AtomicU64,
    }

    #[async_trait]
    impl StreamAttachmentConsumerProbe for CascadeConsumerProbe {
        async fn status(
            &self,
            _key: &StreamAttachmentKeyV1,
        ) -> Result<ConsumerAttachmentStatus, DurableStreamProducerError> {
            Ok(self.status)
        }

        async fn journal_inspection(
            &self,
            _key: &StreamAttachmentKeyV1,
        ) -> Result<Option<ConsumerJournalInspection>, DurableStreamProducerError> {
            Ok(Some(self.inspection.lock().unwrap().clone()))
        }

        async fn commit_source_unavailable(
            &self,
            _key: &StreamAttachmentKeyV1,
            source_offset: StreamOffsetV1,
            consumer_read_ordinal: u64,
        ) -> Result<(), DurableStreamProducerError> {
            let mut inspection = self.inspection.lock().unwrap();
            if inspection.source_offsets.len() as u64 != consumer_read_ordinal {
                return Err(DurableStreamProducerError::CorruptHistory(
                    "test overlay ordinal mismatch".to_string(),
                ));
            }
            match inspection.source_unavailable {
                Some(existing) if existing != source_offset => {
                    return Err(DurableStreamProducerError::CorruptHistory(
                        "test overlay conflict".to_string(),
                    ));
                }
                Some(_) => return Ok(()),
                None => inspection.source_unavailable = Some(source_offset),
            }
            self.overlay_commits.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    struct FailingConsumerProbe {
        failed_stream_id: golem_common::base_model::durable_stream::StreamId,
    }

    #[async_trait]
    impl StreamAttachmentConsumerProbe for FailingConsumerProbe {
        async fn status(
            &self,
            key: &StreamAttachmentKeyV1,
        ) -> Result<ConsumerAttachmentStatus, DurableStreamProducerError> {
            if key.stream_id == self.failed_stream_id {
                Err(DurableStreamProducerError::Oplog(
                    "injected consumer probe failure".to_string(),
                ))
            } else {
                Ok(ConsumerAttachmentStatus::Active)
            }
        }
    }

    struct AdvancingCascadeProbe {
        inspection: std::sync::Mutex<ConsumerJournalInspection>,
        advanced_offset: StreamOffsetV1,
        commits: AtomicU64,
    }

    #[async_trait]
    impl StreamAttachmentConsumerProbe for AdvancingCascadeProbe {
        async fn status(
            &self,
            _key: &StreamAttachmentKeyV1,
        ) -> Result<ConsumerAttachmentStatus, DurableStreamProducerError> {
            Ok(ConsumerAttachmentStatus::Active)
        }

        async fn journal_inspection(
            &self,
            _key: &StreamAttachmentKeyV1,
        ) -> Result<Option<ConsumerJournalInspection>, DurableStreamProducerError> {
            Ok(Some(self.inspection.lock().unwrap().clone()))
        }

        async fn commit_source_unavailable(
            &self,
            _key: &StreamAttachmentKeyV1,
            source_offset: StreamOffsetV1,
            consumer_read_ordinal: u64,
        ) -> Result<(), DurableStreamProducerError> {
            let attempt = self.commits.fetch_add(1, Ordering::Relaxed);
            let mut inspection = self.inspection.lock().unwrap();
            if attempt == 0 {
                inspection.source_offsets.push(self.advanced_offset);
                return Err(DurableStreamProducerError::ConsumerJournalAdvanced);
            }
            if inspection.source_offsets.len() as u64 != consumer_read_ordinal {
                return Err(DurableStreamProducerError::ConsumerJournalAdvanced);
            }
            inspection.source_unavailable = Some(source_offset);
            Ok(())
        }
    }

    struct AmbiguousOverlayCommitProbe {
        inspection: std::sync::Mutex<ConsumerJournalInspection>,
        commits: AtomicU64,
    }

    #[async_trait]
    impl StreamAttachmentConsumerProbe for AmbiguousOverlayCommitProbe {
        async fn status(
            &self,
            _key: &StreamAttachmentKeyV1,
        ) -> Result<ConsumerAttachmentStatus, DurableStreamProducerError> {
            Ok(ConsumerAttachmentStatus::Active)
        }

        async fn journal_inspection(
            &self,
            _key: &StreamAttachmentKeyV1,
        ) -> Result<Option<ConsumerJournalInspection>, DurableStreamProducerError> {
            Ok(Some(self.inspection.lock().unwrap().clone()))
        }

        async fn commit_source_unavailable(
            &self,
            _key: &StreamAttachmentKeyV1,
            source_offset: StreamOffsetV1,
            _consumer_read_ordinal: u64,
        ) -> Result<(), DurableStreamProducerError> {
            let attempt = self.commits.fetch_add(1, Ordering::Relaxed);
            self.inspection.lock().unwrap().source_unavailable = Some(source_offset);
            if attempt == 0 {
                Err(DurableStreamProducerError::Oplog(
                    "injected response loss after overlay commit".to_string(),
                ))
            } else {
                Ok(())
            }
        }
    }

    async fn producer(
        oplog: Arc<TestOplog>,
        identity: &TestIdentity,
        capacity: Option<usize>,
    ) -> Arc<DurableStreamProducer> {
        DurableStreamProducer::load(
            oplog,
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            capacity,
        )
        .await
        .unwrap()
    }

    async fn reconcile(
        producer: &DurableStreamProducer,
        now_millis: u64,
        probe: &(dyn StreamAttachmentConsumerProbe + Send + Sync),
    ) -> Result<usize, DurableStreamProducerError> {
        producer
            .reconcile_attachments_configured(
                now_millis,
                golem_common::base_model::durable_stream::STREAM_ATTACHMENT_RENEWAL_TARGET_MILLIS,
                golem_common::base_model::durable_stream::STREAM_ATTACHMENT_RECONCILIATION_BATCH_SIZE,
                probe,
            )
            .await
    }

    #[test]
    async fn attachment_lifecycle_is_idempotent_fenced_and_rebuildable() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live = producer(oplog.clone(), &identity, None).await;
        let handle = live
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value;
        let key = attachment_key(&identity, handle.stream_id);

        let prepared = live.prepare_attachment(key.clone(), 100).await.unwrap();
        assert!(!prepared.replayed);
        assert_eq!(prepared.value.state, StreamAttachmentStateV1::Prepared);
        assert_eq!(
            prepared.value.lease_expires_at_millis,
            Some(100 + STREAM_ATTACHMENT_LEASE_TTL_MILLIS)
        );
        assert!(
            live.prepare_attachment(key.clone(), 101)
                .await
                .unwrap()
                .replayed
        );
        assert_eq!(oplog.committed_length(), 2);
        assert_eq!(
            live.read_attached_segment(&key, &handle, 102, None, None)
                .await,
            Err(DurableStreamProducerError::InvalidAttachmentState)
        );

        let mut malformed = key.clone();
        malformed.epoch = 0;
        assert_eq!(
            live.activate_attachment(malformed, 110).await,
            Err(DurableStreamProducerError::CorruptHistory(
                "unsupported or malformed durable attachment record".to_string()
            ))
        );
        let mut future = key.clone();
        future.epoch = 2;
        assert_eq!(
            live.activate_attachment(future, 110).await,
            Err(DurableStreamProducerError::InvalidEpoch {
                current: 1,
                actual: 2,
            })
        );

        assert!(
            !live
                .activate_attachment(key.clone(), 120)
                .await
                .unwrap()
                .replayed
        );
        let after_activate = oplog.committed_length();
        assert!(
            live.activate_attachment(key.clone(), 120)
                .await
                .unwrap()
                .replayed
        );
        assert_eq!(oplog.committed_length(), after_activate);
        assert!(
            live.read_attached_segment(&key, &handle, 121, None, None)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            live.read_attached_segment(
                &key,
                &handle,
                120 + STREAM_ATTACHMENT_LEASE_TTL_MILLIS,
                None,
                None,
            )
            .await,
            Err(DurableStreamProducerError::LeaseExpired)
        );
        assert!(
            !live
                .renew_attachment(key.clone(), 130)
                .await
                .unwrap()
                .replayed
        );
        let after_renew = oplog.committed_length();
        assert!(
            live.renew_attachment(key.clone(), 130)
                .await
                .unwrap()
                .replayed
        );
        assert_eq!(oplog.committed_length(), after_renew);
        assert!(
            !live
                .finalize_attachment(
                    key.clone(),
                    StreamAttachmentFinalizationReasonV1::ConsumerFinalized,
                    140,
                )
                .await
                .unwrap()
                .replayed
        );
        let after_finalize = oplog.committed_length();
        assert!(
            live.finalize_attachment(
                key.clone(),
                StreamAttachmentFinalizationReasonV1::ConsumerFinalized,
                140,
            )
            .await
            .unwrap()
            .replayed
        );
        assert_eq!(oplog.committed_length(), after_finalize);
        live.commit_deletion_barrier(1_000, true).await.unwrap();
        drop(live);

        let restarted = producer(oplog, &identity, None).await;
        let attachments = restarted.inspect_attachments().await;
        let [view] = attachments.as_slice() else {
            panic!("restarted producer must rebuild exactly one attachment")
        };
        assert_eq!(view.key, key);
        assert_eq!(
            view.state,
            StreamAttachmentStateV1::Finalized(
                StreamAttachmentFinalizationReasonV1::ConsumerFinalized
            )
        );
    }

    #[test]
    async fn replacing_a_source_cancellation_fences_the_old_drain_without_losing_the_new_one() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog, &identity, None).await;
        let handle = producer
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value;
        let old = CancellationToken::new();
        let old_id = producer.register_source_cancellation(handle.stream_id, old.clone());
        let current = CancellationToken::new();
        let current_id = producer.register_source_cancellation(handle.stream_id, current.clone());

        assert!(old.is_cancelled());
        assert!(!current.is_cancelled());
        producer.unregister_source_cancellation(handle.stream_id, old_id);
        producer
            .cancel_open(
                handle.stream_id,
                StreamCancelRoleV1::OutputConsumer,
                StreamCancelReasonV1::GuestDrop,
                None,
            )
            .await
            .unwrap();
        assert!(current.is_cancelled());
        producer.unregister_source_cancellation(handle.stream_id, current_id);
    }

    #[test]
    async fn producer_rejects_handles_with_altered_non_identity_metadata_before_attachment() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live = producer(oplog.clone(), &identity, None).await;
        let handle = live
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value;
        let registration_length = oplog.current_oplog_index().await;

        let mut altered_source = handle.clone();
        altered_source.source_invocation.idempotency_key =
            IdempotencyKey::new("altered-source".to_string());
        let mut altered_revision = handle.clone();
        altered_revision.component_revision = ComponentRevision::new(2).unwrap();
        let mut altered_schema = handle;
        altered_schema.element_schema_fingerprint = SchemaFingerprintV1([8; 32]);

        for altered in [altered_source, altered_revision, altered_schema] {
            assert_eq!(
                live.validate_handle(&altered).await,
                Err(DurableStreamProducerError::InvalidHandle)
            );
        }
        assert_eq!(oplog.current_oplog_index().await, registration_length);
        assert!(live.inspect_attachments().await.is_empty());
    }

    #[test]
    async fn deletion_is_fail_closed_for_prepared_active_and_expired_references() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live = producer(oplog, &identity, None).await;
        let first = live
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value;
        let key = attachment_key(&identity, first.stream_id);
        live.prepare_attachment(key.clone(), 100).await.unwrap();
        assert!(matches!(
            live.commit_deletion_barrier(1_000, true).await,
            Err(DurableStreamProducerError::DeletionBlocked(ref dependents))
                if dependents == std::slice::from_ref(&key)
        ));
        live.activate_attachment(key.clone(), 110).await.unwrap();
        assert_eq!(
            live.read_attached_segment(
                &key,
                &first,
                110 + STREAM_ATTACHMENT_LEASE_TTL_MILLIS,
                None,
                None,
            )
            .await,
            Err(DurableStreamProducerError::LeaseExpired)
        );
        assert!(matches!(
            live.commit_deletion_barrier(1_000, true).await,
            Err(DurableStreamProducerError::DeletionBlocked(ref dependents))
                if dependents == std::slice::from_ref(&key)
        ));
        live.finalize_attachment(
            key,
            StreamAttachmentFinalizationReasonV1::ConsumerFinalized,
            200,
        )
        .await
        .unwrap();
        live.commit_deletion_barrier(1_000, true).await.unwrap();

        let second_session = StreamInvocationIdV1 {
            idempotency_key: IdempotencyKey::new("second".to_string()),
            ..identity.invocation.clone()
        };
        assert_eq!(
            live.register(ProducerRegistrationRequestV1 {
                coordinate: StreamRegistrationCoordinateV1::Root {
                    invocation_id: second_session.clone(),
                    root_kind: StreamRootKindV1::MethodResult,
                    recursive_value_path: Vec::new(),
                },
                source_invocation: second_session,
                component_revision: ComponentRevision::INITIAL,
                element_schema_fingerprint: SchemaFingerprintV1([7; 32]),
                source_kind: StreamSourceKindV1::InvocationOutput,
                session_mapping: None,
            })
            .await,
            Err(DurableStreamProducerError::ProducerDeleting)
        );
    }

    #[test]
    async fn deletion_gate_and_attachment_prepare_have_one_linearization_order() {
        for _ in 0..16 {
            let identity = identity();
            let live = producer(Arc::new(TestOplog::default()), &identity, None).await;
            let handle = live
                .register(root_registration(&identity))
                .await
                .unwrap()
                .value;
            let key = attachment_key(&identity, handle.stream_id);
            let barrier = Arc::new(Barrier::new(3));
            let deletion = {
                let live = live.clone();
                let barrier = barrier.clone();
                tokio::spawn(async move {
                    barrier.wait().await;
                    live.commit_deletion_barrier(1_000, true).await
                })
            };
            let prepare = {
                let live = live.clone();
                let barrier = barrier.clone();
                let key = key.clone();
                tokio::spawn(async move {
                    barrier.wait().await;
                    live.prepare_attachment(key, 100).await
                })
            };
            barrier.wait().await;

            match (deletion.await.unwrap(), prepare.await.unwrap()) {
                (Ok(()), Err(DurableStreamProducerError::ProducerDeleting)) => {}
                (Err(DurableStreamProducerError::DeletionBlocked(dependents)), Ok(prepared)) => {
                    assert_eq!(dependents, vec![key]);
                    assert_eq!(prepared.value.state, StreamAttachmentStateV1::Prepared);
                }
                outcome => panic!("deletion/prepare race was not linearized: {outcome:?}"),
            }
        }
    }

    #[test]
    async fn deleting_producer_restarts_without_renewing_before_cascade_retry() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live = producer(oplog.clone(), &identity, None).await;
        let handle = live
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value;
        let first_offset = live
            .write_items(handle.stream_id, 0, StreamItemsPayloadV1::PackedU8(vec![7]))
            .await
            .unwrap()
            .value[0];
        let key = attachment_key(&identity, handle.stream_id);
        live.prepare_attachment(key.clone(), 100).await.unwrap();
        live.activate_attachment(key.clone(), 110).await.unwrap();
        live.commit_deletion_barrier(200, false).await.unwrap();
        let after_barrier = oplog.committed_length();
        live.commit_deletion_barrier(200, false).await.unwrap();
        assert_eq!(oplog.committed_length(), after_barrier);
        drop(live);

        let restarted = producer(oplog, &identity, None).await;
        let probe = CascadeConsumerProbe {
            status: ConsumerAttachmentStatus::Active,
            inspection: std::sync::Mutex::new(ConsumerJournalInspection {
                source_offsets: Vec::new(),
                source_unavailable: None,
            }),
            overlay_commits: AtomicU64::new(0),
        };
        assert_eq!(
            restarted
                .reconcile_attachments_configured(1_000, 1, 256, &probe)
                .await
                .unwrap(),
            0
        );
        restarted.cascade_deletion(1_001, &probe).await.unwrap();
        assert_eq!(probe.overlay_commits.load(Ordering::Relaxed), 1);
        assert_eq!(
            restarted.deletion_diagnostics().await.cascade_completed,
            vec![(
                key,
                StreamCascadeDependentResultV1::SourceUnavailable {
                    first_unjournaled_offset: first_offset,
                },
            )]
        );
    }

    #[test]
    async fn cascade_is_durable_idempotent_and_overlays_the_first_unjournaled_position() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live = producer(oplog.clone(), &identity, None).await;
        let handle = live
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value;
        let item_offset = live
            .write_items(handle.stream_id, 0, StreamItemsPayloadV1::PackedU8(vec![7]))
            .await
            .unwrap()
            .value[0];
        let key = attachment_key(&identity, handle.stream_id);
        live.prepare_attachment(key.clone(), 100).await.unwrap();
        live.activate_attachment(key.clone(), 110).await.unwrap();
        let probe = CascadeConsumerProbe {
            status: ConsumerAttachmentStatus::Active,
            inspection: std::sync::Mutex::new(ConsumerJournalInspection {
                source_offsets: Vec::new(),
                source_unavailable: None,
            }),
            overlay_commits: AtomicU64::new(0),
        };

        live.cascade_deletion(200, &probe).await.unwrap();
        assert_eq!(probe.overlay_commits.load(Ordering::Relaxed), 1);
        assert_eq!(
            probe.inspection.lock().unwrap().source_unavailable,
            Some(item_offset)
        );
        let committed_length = oplog.committed_length();
        live.cascade_deletion(201, &probe).await.unwrap();
        assert_eq!(probe.overlay_commits.load(Ordering::Relaxed), 1);
        assert_eq!(oplog.committed_length(), committed_length);
        assert_eq!(
            live.write_items(handle.stream_id, 1, StreamItemsPayloadV1::PackedU8(vec![8]))
                .await,
            Err(DurableStreamProducerError::ProducerDeleting)
        );

        let diagnostics = live.deletion_diagnostics().await;
        assert!(diagnostics.deleting);
        assert_eq!(diagnostics.attachments.len(), 1);
        assert_eq!(
            diagnostics.cascade_completed,
            vec![(
                key.clone(),
                StreamCascadeDependentResultV1::SourceUnavailable {
                    first_unjournaled_offset: item_offset,
                },
            )]
        );
        drop(live);
        let restarted = producer(oplog, &identity, None).await;
        assert_eq!(restarted.deletion_diagnostics().await, diagnostics);
    }

    #[test]
    async fn cascade_retries_when_the_consumer_journal_advances_before_overlay_commit() {
        let identity = identity();
        let live = producer(Arc::new(TestOplog::default()), &identity, None).await;
        let handle = live
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value;
        let offsets = live
            .write_items(
                handle.stream_id,
                0,
                StreamItemsPayloadV1::PackedU8(vec![7, 8]),
            )
            .await
            .unwrap()
            .value;
        let key = attachment_key(&identity, handle.stream_id);
        live.prepare_attachment(key.clone(), 100).await.unwrap();
        live.activate_attachment(key.clone(), 110).await.unwrap();
        let probe = AdvancingCascadeProbe {
            inspection: std::sync::Mutex::new(ConsumerJournalInspection {
                source_offsets: Vec::new(),
                source_unavailable: None,
            }),
            advanced_offset: offsets[0],
            commits: AtomicU64::new(0),
        };

        assert_eq!(
            live.cascade_deletion(200, &probe).await,
            Err(DurableStreamProducerError::ConsumerJournalAdvanced)
        );
        assert!(
            live.deletion_diagnostics()
                .await
                .cascade_completed
                .is_empty()
        );
        live.cascade_deletion(201, &probe).await.unwrap();
        assert_eq!(probe.commits.load(Ordering::Relaxed), 2);
        assert_eq!(
            probe.inspection.lock().unwrap().source_unavailable,
            Some(offsets[1])
        );
        assert_eq!(
            live.deletion_diagnostics().await.cascade_completed,
            vec![(
                key,
                StreamCascadeDependentResultV1::SourceUnavailable {
                    first_unjournaled_offset: offsets[1],
                },
            )]
        );
    }

    #[test]
    async fn cascade_retries_after_overlay_commit_before_outbox_commit() {
        let identity = identity();
        let live = producer(Arc::new(TestOplog::default()), &identity, None).await;
        let handle = live
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value;
        let offset = live
            .write_items(handle.stream_id, 0, StreamItemsPayloadV1::PackedU8(vec![7]))
            .await
            .unwrap()
            .value[0];
        let key = attachment_key(&identity, handle.stream_id);
        live.prepare_attachment(key.clone(), 100).await.unwrap();
        live.activate_attachment(key.clone(), 110).await.unwrap();
        let probe = AmbiguousOverlayCommitProbe {
            inspection: std::sync::Mutex::new(ConsumerJournalInspection {
                source_offsets: Vec::new(),
                source_unavailable: None,
            }),
            commits: AtomicU64::new(0),
        };

        assert!(matches!(
            live.cascade_deletion(200, &probe).await,
            Err(DurableStreamProducerError::Oplog(_))
        ));
        assert_eq!(
            probe.inspection.lock().unwrap().source_unavailable,
            Some(offset)
        );
        assert!(
            live.deletion_diagnostics()
                .await
                .cascade_completed
                .is_empty()
        );
        live.cascade_deletion(201, &probe).await.unwrap();
        assert_eq!(probe.commits.load(Ordering::Relaxed), 1);
        assert_eq!(
            live.deletion_diagnostics().await.cascade_completed,
            vec![(
                key,
                StreamCascadeDependentResultV1::SourceUnavailable {
                    first_unjournaled_offset: offset,
                },
            )]
        );
    }

    #[test]
    async fn source_unavailable_and_consumer_journal_append_are_serialized() {
        let source_identity = identity();
        let source = producer(Arc::new(TestOplog::default()), &source_identity, None).await;
        let handle = source
            .register(root_registration(&source_identity))
            .await
            .unwrap()
            .value;
        let offsets = source
            .write_items(
                handle.stream_id,
                0,
                StreamItemsPayloadV1::PackedU8(vec![7, 8]),
            )
            .await
            .unwrap()
            .value;
        let key = attachment_key(&source_identity, handle.stream_id);
        let consumer_identity = TestIdentity {
            environment_id: key.consumer_environment_id,
            agent_id: key.consumer.clone(),
            fingerprint: key.expected_consumer_fingerprint,
            invocation: key.consumer_invocation.clone(),
        };
        let consumer = producer(Arc::new(TestOplog::default()), &consumer_identity, None).await;
        let first_item =
            StreamSessionRecordV1::ConsumerItemValue(StreamConsumerItemValueRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: key.session_key.clone(),
                stream_id: key.stream_id,
                source_offset: offsets[0],
                consumer_read_ordinal: 0,
                value: vec![7],
                packed_u8: true,
                recursive_handles: Vec::new(),
                recursive_mappings: Vec::new(),
            });
        let barrier = Arc::new(Barrier::new(3));
        let overlay_task = {
            let consumer = consumer.clone();
            let barrier = barrier.clone();
            let key = key.clone();
            let first_offset = offsets[0];
            tokio::spawn(async move {
                barrier.wait().await;
                consumer
                    .commit_source_unavailable_overlay(key, first_offset, 0)
                    .await
            })
        };
        let item_task = {
            let consumer = consumer.clone();
            let barrier = barrier.clone();
            tokio::spawn(async move {
                barrier.wait().await;
                consumer.append_session_record(first_item).await
            })
        };
        barrier.wait().await;
        let overlay_result = overlay_task.await.unwrap();
        let item_result = item_task.await.unwrap();

        match (overlay_result, item_result) {
            (Ok(false), Err(DurableStreamProducerError::ConsumerJournalAdvanced)) => {
                assert!(
                    consumer
                        .commit_source_unavailable_overlay(key.clone(), offsets[0], 0)
                        .await
                        .unwrap()
                );
            }
            (Err(DurableStreamProducerError::ConsumerJournalAdvanced), Ok(())) => {
                assert!(
                    !consumer
                        .commit_source_unavailable_overlay(key.clone(), offsets[1], 1)
                        .await
                        .unwrap()
                );
            }
            results => panic!("journal/overlay race was not linearized: {results:?}"),
        }

        assert_eq!(
            consumer
                .append_session_record(StreamSessionRecordV1::ConsumerItemValue(
                    StreamConsumerItemValueRecordV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: key.session_key,
                        stream_id: key.stream_id,
                        source_offset: offsets[1],
                        consumer_read_ordinal: 1,
                        value: vec![8],
                        packed_u8: true,
                        recursive_handles: Vec::new(),
                        recursive_mappings: Vec::new(),
                    },
                ))
                .await,
            Err(DurableStreamProducerError::ConsumerJournalAdvanced)
        );
    }

    #[test]
    async fn consumer_deleting_intent_fences_prepared_and_activated_topology() {
        let source_identity = identity();
        let source = producer(Arc::new(TestOplog::default()), &source_identity, None).await;
        let handle = source
            .register(root_registration(&source_identity))
            .await
            .unwrap()
            .value;
        let key = attachment_key(&source_identity, handle.stream_id);
        let consumer_identity = TestIdentity {
            environment_id: key.consumer_environment_id,
            agent_id: key.consumer.clone(),
            fingerprint: key.expected_consumer_fingerprint,
            invocation: key.consumer_invocation.clone(),
        };
        let consumer = producer(Arc::new(TestOplog::default()), &consumer_identity, None).await;
        consumer
            .append_session_record(StreamSessionRecordV1::ConsumerDeleting(
                StreamConsumerDeletingRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    consumer_environment_id: consumer_identity.environment_id,
                    consumer: consumer_identity.agent_id,
                    consumer_fingerprint: consumer_identity.fingerprint,
                    deleting_at_millis: 100,
                },
            ))
            .await
            .unwrap();
        let mapping = StreamSessionMappingRecordV1 {
            transport_stream_id: 0,
            handle,
            role: SessionStreamRoleV1::Input,
        };

        assert_eq!(
            consumer
                .append_session_record(StreamSessionRecordV1::TopologyPrepared(
                    StreamTopologyPreparedRecordV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: key.session_key.clone(),
                        attachment: key.clone(),
                        mapping: mapping.clone(),
                    },
                ))
                .await,
            Err(DurableStreamProducerError::ConsumerDeleting)
        );
        assert_eq!(
            consumer
                .append_session_record(StreamSessionRecordV1::TopologyActivated(
                    StreamTopologyActivatedRecordV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: key.session_key.clone(),
                        attachment: key,
                        mapping,
                    },
                ))
                .await,
            Err(DurableStreamProducerError::ConsumerDeleting)
        );
    }

    #[test]
    async fn complete_value_journal_releases_dependency_only_after_the_source_terminal() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live = producer(oplog, &identity, None).await;
        let handle = live
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value;
        let item_offset = live
            .write_items(handle.stream_id, 0, StreamItemsPayloadV1::PackedU8(vec![7]))
            .await
            .unwrap()
            .value[0];
        let terminal_offset = live
            .end(handle.stream_id, 1, StreamEndResultV1::Ok)
            .await
            .unwrap()
            .value;
        let key = attachment_key(&identity, handle.stream_id);
        live.prepare_attachment(key.clone(), 100).await.unwrap();
        live.activate_attachment(key.clone(), 110).await.unwrap();
        let incomplete = CascadeConsumerProbe {
            status: ConsumerAttachmentStatus::Active,
            inspection: std::sync::Mutex::new(ConsumerJournalInspection {
                source_offsets: vec![item_offset],
                source_unavailable: None,
            }),
            overlay_commits: AtomicU64::new(0),
        };
        assert_eq!(reconcile(&live, 130, &incomplete).await.unwrap(), 0);
        assert!(matches!(
            live.inspect_attachments().await[0].state,
            StreamAttachmentStateV1::Active
        ));

        let complete = CascadeConsumerProbe {
            status: ConsumerAttachmentStatus::Active,
            inspection: std::sync::Mutex::new(ConsumerJournalInspection {
                source_offsets: vec![item_offset, terminal_offset],
                source_unavailable: None,
            }),
            overlay_commits: AtomicU64::new(0),
        };
        assert_eq!(reconcile(&live, 150, &complete).await.unwrap(), 1);
        assert_eq!(
            live.inspect_attachments().await[0].state,
            StreamAttachmentStateV1::Finalized(
                StreamAttachmentFinalizationReasonV1::ConsumerFinalized
            )
        );
        live.commit_deletion_barrier(1_000, true).await.unwrap();
    }

    #[test]
    async fn reconciliation_adopts_rolls_back_and_fences_recreated_consumers() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live = producer(oplog, &identity, None).await;
        let handle = live
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value;
        let key = attachment_key(&identity, handle.stream_id);
        live.prepare_attachment(key.clone(), 100).await.unwrap();

        assert_eq!(
            reconcile(
                &live,
                110,
                &FixedConsumerProbe(ConsumerAttachmentStatus::Active),
            )
            .await
            .unwrap(),
            1
        );
        assert_eq!(
            live.inspect_attachments().await[0].state,
            StreamAttachmentStateV1::Active
        );
        assert_eq!(
            reconcile(
                &live,
                120,
                &FixedConsumerProbe(ConsumerAttachmentStatus::IncarnationMismatch),
            )
            .await
            .unwrap(),
            1
        );
        assert_eq!(
            live.inspect_attachments().await[0].state,
            StreamAttachmentStateV1::Finalized(
                StreamAttachmentFinalizationReasonV1::ConsumerIncarnationChanged
            )
        );

        let second_session = StreamInvocationIdV1 {
            idempotency_key: IdempotencyKey::new("abandoned".to_string()),
            ..identity.invocation.clone()
        };
        let second = live
            .register(ProducerRegistrationRequestV1 {
                coordinate: StreamRegistrationCoordinateV1::Root {
                    invocation_id: second_session.clone(),
                    root_kind: StreamRootKindV1::MethodResult,
                    recursive_value_path: Vec::new(),
                },
                source_invocation: second_session,
                component_revision: ComponentRevision::INITIAL,
                element_schema_fingerprint: SchemaFingerprintV1([7; 32]),
                source_kind: StreamSourceKindV1::InvocationOutput,
                session_mapping: None,
            })
            .await
            .unwrap()
            .value;
        let abandoned = attachment_key(&identity, second.stream_id);
        live.prepare_attachment(abandoned.clone(), 200)
            .await
            .unwrap();
        assert_eq!(
            reconcile(
                &live,
                200 + STREAM_ATTACHMENT_ABANDONED_PREPARE_MILLIS - 1,
                &FixedConsumerProbe(ConsumerAttachmentStatus::Missing),
            )
            .await
            .unwrap(),
            0
        );
        assert_eq!(
            reconcile(
                &live,
                200 + STREAM_ATTACHMENT_ABANDONED_PREPARE_MILLIS,
                &FixedConsumerProbe(ConsumerAttachmentStatus::Missing),
            )
            .await
            .unwrap(),
            1
        );
        assert!(matches!(
            live.attachment_view(&abandoned).await.unwrap().state,
            StreamAttachmentStateV1::Finalized(
                StreamAttachmentFinalizationReasonV1::PrepareAbandoned
            )
        ));
    }

    #[test]
    async fn reconciliation_processes_every_attachment_beyond_one_batch() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live = producer(oplog, &identity, None).await;
        let attachment_count =
            golem_common::base_model::durable_stream::STREAM_ATTACHMENT_RECONCILIATION_BATCH_SIZE
                + 1;
        for index in 0..attachment_count {
            let handle = live
                .register(ProducerRegistrationRequestV1 {
                    coordinate: StreamRegistrationCoordinateV1::Root {
                        invocation_id: identity.invocation.clone(),
                        root_kind: StreamRootKindV1::MethodResult,
                        recursive_value_path: vec![StreamValuePathStepV1::ListElement(
                            index as u32,
                        )],
                    },
                    ..root_registration(&identity)
                })
                .await
                .unwrap()
                .value;
            live.prepare_attachment(attachment_key(&identity, handle.stream_id), 100)
                .await
                .unwrap();
        }

        let probe = FixedConsumerProbe(ConsumerAttachmentStatus::Active);
        let first_batch = reconcile(&live, 110, &probe).await.unwrap();
        let second_batch = reconcile(&live, 110, &probe).await.unwrap();
        assert_eq!(first_batch + second_batch, attachment_count);
        assert!(
            live.inspect_attachments()
                .await
                .iter()
                .all(|attachment| attachment.state == StreamAttachmentStateV1::Active)
        );
    }

    #[test]
    async fn reconciliation_continues_after_an_earlier_probe_failure() {
        let identity = identity();
        let live = producer(Arc::new(TestOplog::default()), &identity, None).await;
        let mut keys = Vec::new();
        for index in 0..2 {
            let handle = live
                .register(ProducerRegistrationRequestV1 {
                    coordinate: StreamRegistrationCoordinateV1::Root {
                        invocation_id: identity.invocation.clone(),
                        root_kind: StreamRootKindV1::MethodResult,
                        recursive_value_path: vec![StreamValuePathStepV1::ListElement(index)],
                    },
                    ..root_registration(&identity)
                })
                .await
                .unwrap()
                .value;
            let key = attachment_key(&identity, handle.stream_id);
            live.prepare_attachment(key.clone(), 100).await.unwrap();
            keys.push(key);
        }
        keys.sort_by_key(|key| (key.stream_id, key.attachment_id, key.epoch));

        assert!(
            reconcile(
                &live,
                110,
                &FailingConsumerProbe {
                    failed_stream_id: keys[0].stream_id,
                },
            )
            .await
            .is_err()
        );
        assert_eq!(
            live.attachment_view(&keys[0]).await.unwrap().state,
            StreamAttachmentStateV1::Prepared
        );
        assert_eq!(
            live.attachment_view(&keys[1]).await.unwrap().state,
            StreamAttachmentStateV1::Active
        );
    }

    #[test]
    async fn session_record_commit_folds_a_pending_invocation_added_immediately_before_it() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let committed_batches = Arc::new(Mutex::new(Vec::<Vec<OplogEntry>>::new()));
        let oplog_for_commit = oplog.clone();
        let batches_for_commit = committed_batches.clone();
        let commit: DurableStreamCommit = Arc::new(move |committed| {
            let oplog = oplog_for_commit.clone();
            let batches = batches_for_commit.clone();
            Box::pin(async move {
                let committed_entries = oplog.commit(CommitLevel::Always).await;
                batches
                    .lock()
                    .unwrap()
                    .push(committed_entries.into_values().collect());
                if let Some(committed) = committed {
                    let _ = committed.send(());
                }
            })
        });
        let producer = DurableStreamProducer::load_with_commit(
            oplog.clone(),
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            None,
            commit,
        )
        .await
        .unwrap();
        let stream_id = producer
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value
            .stream_id;
        committed_batches.lock().unwrap().clear();

        oplog
            .add(OplogEntry::pending_agent_invocation(
                IdempotencyKey::new("pending-before-consumer-journal".to_string()),
                OplogPayload::Inline(Box::new(AgentInvocationPayload::SaveSnapshot)),
                TraceId::generate(),
                Vec::new(),
                Vec::new(),
            ))
            .await;
        producer
            .append_session_record(StreamSessionRecordV1::ConsumerTerminal(
                golem_common::model::durable_stream::StreamConsumerTerminalRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: identity.invocation,
                    stream_id,
                    source_offset: StreamOffsetV1::new(OplogIndex::INITIAL, 0),
                    consumer_read_ordinal: 0,
                    terminal: golem_common::model::durable_stream::StreamConsumerTerminalV1::End(
                        StreamEndResultV1::Ok,
                    ),
                },
            ))
            .await
            .unwrap();

        let batches = committed_batches.lock().unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 2);
        assert!(matches!(
            batches[0][0],
            OplogEntry::PendingAgentInvocation { .. }
        ));
        assert!(matches!(batches[0][1], OplogEntry::StreamSession { .. }));
    }

    #[test]
    async fn producer_journal_restarts_and_replays_without_appending() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live_producer = producer(oplog.clone(), &identity, None).await;
        let registered = live_producer
            .register(root_registration(&identity))
            .await
            .unwrap();
        assert!(!registered.replayed);
        let stream_id = registered.value.stream_id;
        assert_eq!(
            live_producer.input_high_water(stream_id).await.unwrap(),
            None
        );

        let written = live_producer
            .write_items(stream_id, 0, StreamItemsPayloadV1::PackedU8(vec![10, 11]))
            .await
            .unwrap();
        assert_eq!(written.value.len(), 2);
        assert_eq!(
            written.value[0].producer_oplog_index(),
            OplogIndex::from_u64(2)
        );
        assert_eq!(written.value[0].sub_index(), 0);
        assert_eq!(written.value[1].sub_index(), 1);
        assert_eq!(
            live_producer.input_high_water(stream_id).await.unwrap(),
            Some(InputStreamHighWaterV1 {
                highest_contiguous_sequence: 1,
                resulting_offset: written.value[1],
                terminal: false,
            })
        );
        let terminal = live_producer
            .end(stream_id, 2, StreamEndResultV1::Ok)
            .await
            .unwrap();
        assert_eq!(
            terminal.value.producer_oplog_index(),
            OplogIndex::from_u64(3)
        );
        assert_eq!(
            live_producer.input_high_water(stream_id).await.unwrap(),
            Some(InputStreamHighWaterV1 {
                highest_contiguous_sequence: 2,
                resulting_offset: terminal.value,
                terminal: true,
            })
        );
        assert_eq!(oplog.committed_length(), 3);

        drop(live_producer);
        let restarted = producer(oplog.clone(), &identity, None).await;
        let replayed_registration = restarted
            .register(root_registration(&identity))
            .await
            .unwrap();
        assert!(replayed_registration.replayed);
        assert_eq!(replayed_registration.value, registered.value);
        assert!(
            restarted
                .write_items(stream_id, 0, StreamItemsPayloadV1::PackedU8(vec![10, 11]))
                .await
                .unwrap()
                .replayed
        );
        assert!(
            restarted
                .end(stream_id, 2, StreamEndResultV1::Ok)
                .await
                .unwrap()
                .replayed
        );
        assert_eq!(
            restarted.input_high_water(stream_id).await.unwrap(),
            Some(InputStreamHighWaterV1 {
                highest_contiguous_sequence: 2,
                resulting_offset: terminal.value,
                terminal: true,
            })
        );
        assert_eq!(oplog.committed_length(), 3);

        let mut reader = restarted
            .catch_up(registered.value, Some(written.value[0]))
            .await
            .unwrap();
        assert_eq!(
            reader.next().await.unwrap().unwrap().payload,
            CommittedProducerStreamEventPayloadV1::PackedU8(11)
        );
        assert_eq!(
            reader.next().await.unwrap().unwrap().payload,
            CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::Ok)
        );
        assert!(reader.next().await.unwrap().is_none());
    }

    #[test]
    async fn producer_end_after_earlier_input_consumer_cancel_is_fenced() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog.clone(), &identity, None).await;
        let handle = producer
            .register(registration(
                &identity,
                StreamRegistrationCoordinateV1::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodInput,
                    recursive_value_path: Vec::new(),
                },
                StreamSourceKindV1::ExternalInlineInput,
            ))
            .await
            .unwrap()
            .value;
        producer
            .write_items(
                handle.stream_id,
                0,
                StreamItemsPayloadV1::Values(vec![vec![1]]),
            )
            .await
            .unwrap();
        producer
            .cancel_open(
                handle.stream_id,
                StreamCancelRoleV1::InputConsumer,
                StreamCancelReasonV1::GuestDrop,
                None,
            )
            .await
            .unwrap();
        let oplog_length = oplog.committed_length();

        assert!(matches!(
            producer
                .end(handle.stream_id, 64, StreamEndResultV1::Ok)
                .await,
            Err(DurableStreamProducerError::FencedByTerminal(
                CommittedProducerStreamEventPayloadV1::Cancel {
                    role: StreamCancelRoleV1::InputConsumer,
                    reason: StreamCancelReasonV1::GuestDrop,
                    details: None,
                }
            ))
        ));
        assert_eq!(oplog.committed_length(), oplog_length);
    }

    #[test]
    async fn prepared_input_registration_batch_recovers_without_duplicate_registration() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let commit_reached = Arc::new(Barrier::new(2));
        let commit: DurableStreamCommit = Arc::new({
            let oplog = oplog.clone();
            let commit_reached = commit_reached.clone();
            move |committed| {
                let oplog = oplog.clone();
                let commit_reached = commit_reached.clone();
                Box::pin(async move {
                    oplog.commit(CommitLevel::Always).await;
                    if let Some(committed) = committed {
                        let _ = committed.send(());
                    }
                    commit_reached.wait().await;
                    std::future::pending::<()>().await;
                })
            }
        });
        let live_producer = DurableStreamProducer::load_with_commit(
            oplog.clone(),
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            None,
            commit,
        )
        .await
        .unwrap();
        let registration = registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodInput,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::ExternalInlineInput,
        );
        let session_key = identity.invocation.clone();
        let callee_fingerprint = identity.fingerprint;
        let attachment_id = AttachmentId::primary(
            session_key.callee_environment_id,
            &session_key.callee,
            &session_key.idempotency_key,
        )
        .unwrap();
        let attempt_id = AttemptId(Uuid::new_v4());
        let pending = OplogEntry::pending_agent_invocation(
            IdempotencyKey::new("durable-session".to_string()),
            OplogPayload::Inline(Box::new(AgentInvocationPayload::SaveSnapshot)),
            TraceId::generate(),
            Vec::new(),
            Vec::new(),
        );
        let (committed, committed_rx) = oneshot::channel();
        let preparation = tokio::spawn({
            let live_producer = live_producer.clone();
            let registration = registration.clone();
            async move {
                live_producer
                    .prepare_session(
                        vec![(17, registration)],
                        pending,
                        committed,
                        move |bindings| {
                            let handles = bindings
                                .iter()
                                .map(|(_, handle)| handle.clone())
                                .collect::<Vec<_>>();
                            StreamSessionPreparedRecordV1 {
                                format_version: DURABLE_STREAM_FORMAT_VERSION,
                                attempt: StartAttemptDescriptorV1 {
                                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                                    session_key: session_key.clone(),
                                    attachment_id,
                                    expected_callee_fingerprint: callee_fingerprint,
                                    attempt_id,
                                    invocation: PersistedStreamInvocationDescriptorV1 {
                                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                                        session_key,
                                        target_component_revision: ComponentRevision::INITIAL,
                                        method_name: "consume".to_string(),
                                        invocation_value: vec![1],
                                        stream_handles: handles,
                                        execution_config: vec![2],
                                        effective_identity: vec![3],
                                    },
                                    effective_identity: vec![3],
                                    live_join_buffer_events: 8,
                                },
                                stream_mappings: bindings
                                    .into_iter()
                                    .map(|(transport_stream_id, handle)| {
                                        StreamSessionMappingRecordV1 {
                                            transport_stream_id,
                                            handle,
                                            role: SessionStreamRoleV1::Input,
                                        }
                                    })
                                    .collect(),
                            }
                        },
                    )
                    .await
            }
        });
        committed_rx.await.unwrap();
        commit_reached.wait().await;
        assert_eq!(oplog.committed_length(), 4);
        assert_eq!(oplog.commit_count(), 1);
        preparation.abort();
        assert!(preparation.await.unwrap_err().is_cancelled());
        drop(live_producer);

        let entries = oplog.entries();
        assert!(matches!(entries[0], OplogEntry::StreamRegistered { .. }));
        let OplogEntry::StreamSession {
            record: OplogPayload::Inline(prepared),
            ..
        } = &entries[1]
        else {
            panic!("acceptance batch must contain an inline Prepared record");
        };
        let StreamSessionRecordV1::Prepared(prepared) = prepared.as_ref() else {
            panic!("acceptance batch must contain a Prepared record");
        };
        assert!(matches!(
            entries[2],
            OplogEntry::PendingAgentInvocation { .. }
        ));
        let OplogEntry::StreamSession {
            record: OplogPayload::Inline(attached),
            ..
        } = &entries[3]
        else {
            panic!("acceptance batch must end with an inline Attached record");
        };
        let StreamSessionRecordV1::Attached(attached) = attached.as_ref() else {
            panic!("acceptance batch must end with an Attached record");
        };
        assert_eq!(attached.pending_invocation_oplog_index.as_u64(), 3);
        assert_eq!(prepared.stream_mappings.len(), 1);

        let restarted = producer(oplog.clone(), &identity, None).await;
        let recovered = restarted
            .validate_registration(&registration)
            .await
            .unwrap();
        assert_eq!(recovered, prepared.stream_mappings[0].handle);
        assert_eq!(oplog.committed_length(), 4);
    }

    #[test]
    async fn protocol_terminalization_closes_an_open_stream_once() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog.clone(), &identity, None).await;
        let handle = producer
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value;
        producer
            .write_items(
                handle.stream_id,
                0,
                StreamItemsPayloadV1::Values(vec![vec![42]]),
            )
            .await
            .unwrap();

        producer
            .end_open(
                handle.stream_id,
                StreamEndResultV1::ErrorContext(b"invocation failed".to_vec()),
            )
            .await
            .unwrap();
        let committed_length = oplog.committed_length();
        producer
            .end_open(
                handle.stream_id,
                StreamEndResultV1::ErrorContext(b"ignored duplicate".to_vec()),
            )
            .await
            .unwrap();

        assert_eq!(oplog.committed_length(), committed_length);
        let mut reader = producer.catch_up(handle, None).await.unwrap();
        assert!(matches!(
            reader.next().await.unwrap().unwrap().payload,
            CommittedProducerStreamEventPayloadV1::Value(_)
        ));
        assert!(matches!(
            reader.next().await.unwrap().unwrap().payload,
            CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::ErrorContext(details))
                if details == b"invocation failed"
        ));
        assert!(reader.next().await.unwrap().is_none());
    }

    #[test]
    async fn empty_invocation_result_replays_exactly_and_rejects_conflicts() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog.clone(), &identity, None).await;
        let result = |payload: Vec<u8>| {
            StreamSessionRecordV1::InvocationResult(StreamSessionInvocationResultRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: identity.invocation.clone(),
                result: payload,
                output_streams: Vec::new(),
                stream_mappings: Vec::new(),
            })
        };

        producer
            .register_result_streams(Vec::new(), {
                let record = result(vec![1]);
                move |_| record
            })
            .await
            .unwrap();
        let committed = oplog.committed_length();

        producer
            .register_result_streams(Vec::new(), {
                let record = result(vec![1]);
                move |_| record
            })
            .await
            .unwrap();
        assert_eq!(oplog.committed_length(), committed);

        assert_eq!(
            producer
                .register_result_streams(Vec::new(), {
                    let record = result(vec![2]);
                    move |_| record
                })
                .await
                .unwrap_err(),
            DurableStreamProducerError::RegistrationDivergence
        );
        assert_eq!(oplog.committed_length(), committed);
    }

    #[test]
    async fn nested_registration_and_enclosing_item_share_one_ordered_batch() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog.clone(), &identity, None).await;
        let parent = producer
            .register(root_registration(&identity))
            .await
            .unwrap();
        let nested = registration(
            &identity,
            StreamRegistrationCoordinateV1::Nested {
                parent_stream_id: parent.value.stream_id,
                parent_producer_sequence: 0,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::Nested,
        );
        let written = producer
            .write_items_with_nested(
                parent.value.stream_id,
                0,
                StreamItemsPayloadV1::Values(vec![vec![1, 2, 3]]),
                vec![nested.clone()],
            )
            .await
            .unwrap();
        assert_eq!(
            written.value[0].producer_oplog_index(),
            OplogIndex::from_u64(3)
        );
        let nested_replay = producer.register(nested).await.unwrap();
        assert!(nested_replay.replayed);
        assert_eq!(oplog.committed_length(), 3);
        assert!(
            producer
                .write_items_with_nested(
                    parent.value.stream_id,
                    0,
                    StreamItemsPayloadV1::Values(vec![vec![1, 2, 3]]),
                    vec![registration(
                        &identity,
                        StreamRegistrationCoordinateV1::Nested {
                            parent_stream_id: parent.value.stream_id,
                            parent_producer_sequence: 0,
                            recursive_value_path: Vec::new(),
                        },
                        StreamSourceKindV1::Nested,
                    )],
                )
                .await
                .unwrap()
                .replayed
        );
        assert_eq!(oplog.committed_length(), 3);
        let entries = oplog.read_exact(OplogIndex::INITIAL, 3).await;
        assert!(matches!(
            entries.get(&OplogIndex::from_u64(2)),
            Some(OplogEntry::StreamRegistered { .. })
        ));
        assert!(matches!(
            entries.get(&OplogIndex::from_u64(3)),
            Some(OplogEntry::StreamItems { .. })
        ));
    }

    #[test]
    async fn new_nested_registration_cannot_commit_without_its_enclosing_item() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog.clone(), &identity, None).await;
        let parent = producer
            .register(root_registration(&identity))
            .await
            .unwrap();
        let nested = registration(
            &identity,
            StreamRegistrationCoordinateV1::Nested {
                parent_stream_id: parent.value.stream_id,
                parent_producer_sequence: 0,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::Nested,
        );

        assert_eq!(
            producer.register(nested).await,
            Err(DurableStreamProducerError::RegistrationDivergence)
        );
        assert_eq!(
            oplog.committed_length(),
            1,
            "same-producer nested registration must only commit in the enclosing item's batch"
        );
    }

    #[test]
    async fn catch_up_joins_live_without_a_gap_or_duplicate() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog, &identity, None).await;
        let registered = producer
            .register(root_registration(&identity))
            .await
            .unwrap();
        let mut reader = producer
            .catch_up(registered.value.clone(), None)
            .await
            .unwrap();
        producer
            .write_items(
                registered.value.stream_id,
                0,
                StreamItemsPayloadV1::Values(vec![vec![9]]),
            )
            .await
            .unwrap();
        producer
            .end(registered.value.stream_id, 1, StreamEndResultV1::Ok)
            .await
            .unwrap();
        assert_eq!(
            reader.next().await.unwrap().unwrap().payload,
            CommittedProducerStreamEventPayloadV1::Value(vec![9])
        );
        assert!(matches!(
            reader.next().await.unwrap().unwrap().payload,
            CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::Ok)
        ));
        assert!(reader.next().await.unwrap().is_none());
    }

    #[test]
    async fn replay_publication_is_deduplicated_at_the_live_reader() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog, &identity, None).await;
        let registered = producer
            .register(root_registration(&identity))
            .await
            .unwrap();
        let mut reader = producer
            .catch_up(registered.value.clone(), None)
            .await
            .unwrap();
        producer
            .write_items(
                registered.value.stream_id,
                0,
                StreamItemsPayloadV1::Values(vec![vec![1]]),
            )
            .await
            .unwrap();
        assert_eq!(reader.next().await.unwrap().unwrap().producer_sequence, 0);

        assert!(
            producer
                .write_items(
                    registered.value.stream_id,
                    0,
                    StreamItemsPayloadV1::Values(vec![vec![1]]),
                )
                .await
                .unwrap()
                .replayed
        );
        producer
            .end(registered.value.stream_id, 1, StreamEndResultV1::Ok)
            .await
            .unwrap();
        let terminal = reader.next().await.unwrap().unwrap();
        assert_eq!(terminal.producer_sequence, 1);
        assert!(matches!(
            terminal.payload,
            CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::Ok)
        ));
    }

    #[test]
    async fn malformed_history_is_rejected_while_rebuilding_the_index() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog.clone(), &identity, None).await;
        let registered = producer
            .register(root_registration(&identity))
            .await
            .unwrap();
        let stream_id = registered.value.stream_id;
        let producer_fingerprint = identity.fingerprint;
        oplog
            .add_durable_stream_batch(Box::new(move |item_index| {
                vec![DurableStreamOplogRecord::Items(StreamItemsRecordV1 {
                    format_version: 1,
                    stream_id,
                    producer_fingerprint,
                    first_sequence: 1,
                    nested_stream_ids: Vec::new(),
                    newly_registered_stream_ids: Vec::new(),
                    payload: StreamItemsPayloadV1::Values(vec![vec![1]]),
                    offsets: vec![StreamOffsetV1::new(item_index, 0)],
                })]
            }))
            .await
            .unwrap();
        oplog.commit(CommitLevel::Always).await;
        drop(producer);

        assert!(matches!(
            DurableStreamProducer::load(
                oplog,
                identity.environment_id,
                identity.agent_id,
                identity.fingerprint,
                None,
            )
            .await,
            Err(super::DurableStreamProducerError::SequenceGap {
                expected: 0,
                actual: 1,
            })
        ));
    }

    #[test]
    async fn rejected_nested_item_batch_does_not_partially_mutate_the_stream_index() {
        let identity = identity();
        let mut index = ProducerStreamIndex::default();
        let root_index = OplogIndex::INITIAL;
        let root = registration_record(
            root_index,
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            root_registration(&identity),
        );
        let parent_stream_id = root.handle.stream_id;
        index
            .apply_registration(
                root_index,
                root,
                identity.environment_id,
                &identity.agent_id,
                identity.fingerprint,
            )
            .unwrap();

        let nested_index = root_index.next();
        let nested = registration_record(
            nested_index,
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            registration(
                &identity,
                StreamRegistrationCoordinateV1::Nested {
                    parent_stream_id,
                    parent_producer_sequence: 1,
                    recursive_value_path: vec![StreamValuePathStepV1::OptionSome],
                },
                StreamSourceKindV1::Nested,
            ),
        );
        let nested_stream_id = nested.handle.stream_id;
        let item_index = nested_index.next();
        let error = index
            .apply_item_batch(
                item_index,
                vec![(nested_index, nested)],
                StreamItemsRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    stream_id: parent_stream_id,
                    producer_fingerprint: identity.fingerprint,
                    first_sequence: 1,
                    nested_stream_ids: vec![nested_stream_id],
                    newly_registered_stream_ids: vec![nested_stream_id],
                    payload: StreamItemsPayloadV1::Values(vec![vec![1]]),
                    offsets: vec![StreamOffsetV1::new(item_index, 0)],
                },
                identity.environment_id,
                &identity.agent_id,
                identity.fingerprint,
            )
            .unwrap_err();

        assert_eq!(
            error,
            DurableStreamProducerError::SequenceGap {
                expected: 0,
                actual: 1,
            }
        );
        assert!(!index.registrations.contains_key(&nested_stream_id));
        assert!(!index.streams.contains_key(&nested_stream_id));
        assert_eq!(index.registrations.len(), 1);
    }

    #[test]
    async fn history_rebuild_rejects_duplicate_nested_stream_ownership() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog.clone(), &identity, None).await;
        let parent = producer
            .register(root_registration(&identity))
            .await
            .unwrap();
        let parent_stream_id = parent.value.stream_id;
        let environment_id = identity.environment_id;
        let agent_id = identity.agent_id.clone();
        let producer_fingerprint = identity.fingerprint;
        let nested = registration(
            &identity,
            StreamRegistrationCoordinateV1::Nested {
                parent_stream_id,
                parent_producer_sequence: 0,
                recursive_value_path: vec![StreamValuePathStepV1::OptionSome],
            },
            StreamSourceKindV1::Nested,
        );
        oplog
            .add_durable_stream_batch(Box::new(move |registration_index| {
                let nested_record = registration_record(
                    registration_index,
                    environment_id,
                    agent_id,
                    producer_fingerprint,
                    nested,
                );
                let nested_stream_id = nested_record.handle.stream_id;
                let item_index = OplogIndex::from_u64(registration_index.as_u64() + 1);
                vec![
                    DurableStreamOplogRecord::Registered(nested_record),
                    DurableStreamOplogRecord::Items(StreamItemsRecordV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        stream_id: parent_stream_id,
                        producer_fingerprint,
                        first_sequence: 0,
                        nested_stream_ids: vec![nested_stream_id, nested_stream_id],
                        newly_registered_stream_ids: vec![nested_stream_id],
                        payload: StreamItemsPayloadV1::Values(vec![vec![1]]),
                        offsets: vec![StreamOffsetV1::new(item_index, 0)],
                    }),
                ]
            }))
            .await
            .unwrap();
        oplog.commit(CommitLevel::Always).await;
        drop(producer);

        assert!(
            DurableStreamProducer::load(
                oplog,
                identity.environment_id,
                identity.agent_id,
                identity.fingerprint,
                None,
            )
            .await
            .is_err(),
            "one affine nested stream cannot be owned twice by the same enclosing value"
        );
    }

    #[test]
    async fn history_rebuild_rejects_nested_registration_without_enclosing_item() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog.clone(), &identity, None).await;
        let parent = producer
            .register(root_registration(&identity))
            .await
            .unwrap();
        let environment_id = identity.environment_id;
        let agent_id = identity.agent_id.clone();
        let producer_fingerprint = identity.fingerprint;
        let nested = registration(
            &identity,
            StreamRegistrationCoordinateV1::Nested {
                parent_stream_id: parent.value.stream_id,
                parent_producer_sequence: 0,
                recursive_value_path: vec![StreamValuePathStepV1::OptionSome],
            },
            StreamSourceKindV1::Nested,
        );
        oplog
            .add_durable_stream_batch(Box::new(move |registration_index| {
                vec![DurableStreamOplogRecord::Registered(registration_record(
                    registration_index,
                    environment_id,
                    agent_id,
                    producer_fingerprint,
                    nested,
                ))]
            }))
            .await
            .unwrap();
        oplog.commit(CommitLevel::Always).await;
        drop(producer);

        assert!(matches!(
            DurableStreamProducer::load(
                oplog,
                identity.environment_id,
                identity.agent_id,
                identity.fingerprint,
                None,
            )
            .await,
            Err(DurableStreamProducerError::CorruptHistory(_))
        ));
    }

    #[test]
    async fn encoded_size_rejection_has_no_durable_effect_and_sequence_can_retry() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog.clone(), &identity, None).await;
        let registered = producer
            .register(root_registration(&identity))
            .await
            .unwrap();
        let stream_id = registered.value.stream_id;

        assert_eq!(
            producer
                .write_items(
                    stream_id,
                    0,
                    StreamItemsPayloadV1::Values(vec![vec![0; MAX_DURABLE_STREAM_ITEM_SIZE + 1]]),
                )
                .await,
            Err(DurableStreamProducerError::ItemTooLarge)
        );
        assert_eq!(oplog.committed_length(), 1);
        assert_eq!(
            producer
                .write_items(
                    stream_id,
                    0,
                    StreamItemsPayloadV1::PackedU8(vec![0; MAX_PACKED_U8_STREAM_ITEM_SIZE + 1]),
                )
                .await,
            Err(DurableStreamProducerError::InvalidPackedU8Batch)
        );
        assert_eq!(oplog.committed_length(), 1);

        let written = producer
            .write_items(stream_id, 0, StreamItemsPayloadV1::Values(vec![vec![1]]))
            .await
            .unwrap();
        assert_eq!(
            written.value[0].producer_oplog_index(),
            OplogIndex::from_u64(2)
        );
    }

    #[test]
    async fn root_registration_rejects_coordinate_beyond_traversal_depth_limit() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog.clone(), &identity, None).await;
        let request = registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: (0..=MAX_STREAM_VALUE_TRAVERSAL_DEPTH)
                    .map(|_| StreamValuePathStepV1::OptionSome)
                    .collect(),
            },
            StreamSourceKindV1::InvocationOutput,
        );

        assert_eq!(
            producer.register(request).await,
            Err(DurableStreamProducerError::TraversalDepthLimit)
        );
        assert_eq!(
            oplog.committed_length(),
            0,
            "an invalid initial descriptor must have no durable effect"
        );
    }

    #[test]
    async fn rejects_out_of_range_join_capacity_before_registration() {
        let identity = identity();
        for invalid_capacity in [0, MAX_LIVE_JOIN_BUFFER_SIZE + 1] {
            let oplog = Arc::new(TestOplog::default());
            assert!(matches!(
                DurableStreamProducer::load(
                    oplog.clone(),
                    identity.environment_id,
                    identity.agent_id.clone(),
                    identity.fingerprint,
                    Some(invalid_capacity),
                )
                .await,
                Err(DurableStreamProducerError::LiveBus(
                    DurableLiveStreamBusError::InvalidCapacity
                ))
            ));
            assert_eq!(oplog.committed_length(), 0);
        }
    }

    #[test]
    async fn stream_limit_is_scoped_to_one_session_not_the_producer_agent_lifetime() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog, &identity, None).await;

        for session in 0..=MAX_DURABLE_STREAMS_PER_SESSION {
            let mut invocation = identity.invocation.clone();
            invocation.idempotency_key = IdempotencyKey::new(format!("session-{session}"));
            let request = ProducerRegistrationRequestV1 {
                coordinate: StreamRegistrationCoordinateV1::Root {
                    invocation_id: invocation.clone(),
                    root_kind: StreamRootKindV1::MethodResult,
                    recursive_value_path: Vec::new(),
                },
                source_invocation: invocation,
                component_revision: ComponentRevision::INITIAL,
                element_schema_fingerprint: SchemaFingerprintV1([7; 32]),
                source_kind: StreamSourceKindV1::InvocationOutput,
                session_mapping: None,
            };

            producer.register(request).await.expect(
                "one stream in each independent session must remain below the per-session limit",
            );
        }
    }

    #[test]
    async fn foreign_mappings_are_deduplicated_and_count_toward_the_session_limit() {
        let identity = identity();
        let mapping = |position: usize| StreamSessionMappingRecordV1 {
            transport_stream_id: position as u64,
            handle: golem_common::base_model::durable_stream::DurableStreamHandleV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                stream_id: StreamId(Uuid::from_u128(10_000 + position as u128)),
                producer_environment_id: EnvironmentId(Uuid::from_u128(41)),
                producer: AgentId {
                    component_id: ComponentId(Uuid::from_u128(42)),
                    agent_id: "foreign-producer".to_string(),
                },
                expected_producer_fingerprint: AgentFingerprint(Uuid::from_u128(43)),
                source_invocation: identity.invocation.clone(),
                component_revision: ComponentRevision::INITIAL,
                element_schema_fingerprint: SchemaFingerprintV1([9; 32]),
            },
            role: SessionStreamRoleV1::Input,
        };
        let record = |mapping| {
            StreamSessionRecordV1::Mapping(StreamSessionMappingUpdateRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: identity.invocation.clone(),
                mapping,
            })
        };
        let mut index = ProducerStreamIndex::default();
        for position in 0..MAX_DURABLE_STREAMS_PER_SESSION {
            index
                .apply_session_references(&record(mapping(position)))
                .unwrap();
        }
        assert_eq!(
            index.session_stream_counts[&identity.invocation],
            MAX_DURABLE_STREAMS_PER_SESSION
        );

        index.apply_session_references(&record(mapping(0))).unwrap();
        assert_eq!(
            index.session_stream_counts[&identity.invocation],
            MAX_DURABLE_STREAMS_PER_SESSION
        );
        assert_eq!(
            index.apply_session_references(&record(mapping(MAX_DURABLE_STREAMS_PER_SESSION))),
            Err(DurableStreamProducerError::StreamLimit)
        );
    }

    #[test]
    async fn malformed_session_record_is_rejected_at_the_write_boundary() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog.clone(), &identity, None).await;
        let mut malformed_handle = producer
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value;
        malformed_handle.format_version = DURABLE_STREAM_FORMAT_VERSION + 1;
        let before = oplog.current_oplog_index().await;

        assert!(matches!(
            producer
                .append_session_record(StreamSessionRecordV1::Mapping(
                    StreamSessionMappingUpdateRecordV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: identity.invocation,
                        mapping: StreamSessionMappingRecordV1 {
                            transport_stream_id: 17,
                            handle: malformed_handle,
                            role: SessionStreamRoleV1::Output,
                        },
                    },
                ))
                .await,
            Err(DurableStreamProducerError::CorruptHistory(_))
        ));
        assert_eq!(oplog.current_oplog_index().await, before);
    }

    #[test]
    async fn recursive_value_limit_commits_only_one_protocol_resource_exhausted_terminal() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog.clone(), &identity, None).await;
        let registered = producer
            .register(root_registration(&identity))
            .await
            .unwrap();
        let stream_id = registered.value.stream_id;
        let nested = (0..=MAX_NEW_STREAM_HANDLES_PER_VALUE)
            .map(|position| {
                registration(
                    &identity,
                    StreamRegistrationCoordinateV1::Nested {
                        parent_stream_id: stream_id,
                        parent_producer_sequence: 0,
                        recursive_value_path: vec![StreamValuePathStepV1::ListElement(
                            position as u32,
                        )],
                    },
                    StreamSourceKindV1::Nested,
                )
            })
            .collect();

        assert_eq!(
            producer
                .write_items_with_nested(
                    stream_id,
                    0,
                    StreamItemsPayloadV1::Values(vec![vec![1]]),
                    nested,
                )
                .await,
            Err(DurableStreamProducerError::ValueStreamLimit)
        );
        assert_eq!(oplog.committed_length(), 2);
        assert_eq!(producer.index.lock().await.registrations.len(), 1);

        let mut reader = producer.catch_up(registered.value, None).await.unwrap();
        let terminal = reader.next().await.unwrap().unwrap();
        assert_eq!(terminal.producer_sequence, 0);
        assert_eq!(
            terminal.terminal_author,
            Some(StreamTerminalAuthorV1::Protocol)
        );
        let CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::ErrorContext(bytes)) =
            terminal.payload
        else {
            panic!("expected resource exhaustion stream terminal")
        };
        let error: AgentError = golem_common::serialization::deserialize(&bytes).unwrap();
        assert_eq!(error.to_string(), "\"ResourceExhausted\"");

        assert!(matches!(
            producer
                .write_items(stream_id, 0, StreamItemsPayloadV1::Values(vec![vec![2]]),)
                .await,
            Err(DurableStreamProducerError::FencedByTerminal(_))
        ));
        assert_eq!(oplog.committed_length(), 2);
    }

    #[test]
    async fn traversal_session_and_counter_limits_terminalize_without_partial_items() {
        async fn fresh() -> (
            TestIdentity,
            Arc<TestOplog>,
            Arc<DurableStreamProducer>,
            golem_common::base_model::durable_stream::DurableStreamHandleV1,
        ) {
            let identity = identity();
            let oplog = Arc::new(TestOplog::default());
            let producer = producer(oplog.clone(), &identity, None).await;
            let handle = producer
                .register(root_registration(&identity))
                .await
                .unwrap()
                .value;
            (identity, oplog, producer, handle)
        }

        let (_identity, depth_oplog, depth_producer, depth_handle) = fresh().await;
        assert_eq!(
            depth_producer
                .write_items_with_nested_at_depth(
                    depth_handle.stream_id,
                    0,
                    StreamItemsPayloadV1::Values(vec![vec![1]]),
                    Vec::new(),
                    MAX_STREAM_VALUE_TRAVERSAL_DEPTH + 1,
                )
                .await,
            Err(DurableStreamProducerError::TraversalDepthLimit)
        );
        assert_eq!(depth_oplog.committed_length(), 2);

        let (identity, stream_oplog, stream_producer, stream_handle) = fresh().await;
        {
            let mut index = stream_producer.index.lock().await;
            let session_key = index
                .stream_sessions
                .get(&stream_handle.stream_id)
                .unwrap()
                .clone();
            index
                .session_stream_counts
                .insert(session_key, MAX_DURABLE_STREAMS_PER_SESSION);
        }
        let nested = registration(
            &identity,
            StreamRegistrationCoordinateV1::Nested {
                parent_stream_id: stream_handle.stream_id,
                parent_producer_sequence: 0,
                recursive_value_path: vec![StreamValuePathStepV1::OptionSome],
            },
            StreamSourceKindV1::Nested,
        );
        assert_eq!(
            stream_producer
                .write_items_with_nested(
                    stream_handle.stream_id,
                    0,
                    StreamItemsPayloadV1::Values(vec![vec![1]]),
                    vec![nested],
                )
                .await,
            Err(DurableStreamProducerError::StreamLimit)
        );
        assert_eq!(stream_oplog.committed_length(), 2);

        let (_identity, counter_oplog, counter_producer, counter_handle) = fresh().await;
        counter_producer
            .index
            .lock()
            .await
            .streams
            .get_mut(&counter_handle.stream_id)
            .unwrap()
            .next_sequence = u64::MAX;
        assert_eq!(
            counter_producer
                .write_items(
                    counter_handle.stream_id,
                    u64::MAX,
                    StreamItemsPayloadV1::Values(vec![vec![1]]),
                )
                .await,
            Err(DurableStreamProducerError::CounterOverflow)
        );
        assert_eq!(counter_oplog.committed_length(), 2);
        let mut reader = counter_producer
            .catch_up(counter_handle, None)
            .await
            .unwrap();
        assert_eq!(
            reader.next().await.unwrap().unwrap().producer_sequence,
            u64::MAX
        );
    }

    #[test]
    #[timeout("30s")]
    async fn restart_recovers_registration_committed_before_caller_observation() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let commit_reached = Arc::new(Barrier::new(2));
        let commit: DurableStreamCommit = Arc::new({
            let oplog = oplog.clone();
            let commit_reached = commit_reached.clone();
            move |committed| {
                let oplog = oplog.clone();
                let commit_reached = commit_reached.clone();
                Box::pin(async move {
                    oplog.commit(CommitLevel::Always).await;
                    if let Some(committed) = committed {
                        let _ = committed.send(());
                    }
                    commit_reached.wait().await;
                    std::future::pending::<()>().await;
                })
            }
        });
        let live = DurableStreamProducer::load_with_commit(
            oplog.clone(),
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            None,
            commit,
        )
        .await
        .unwrap();
        let request = root_registration(&identity);
        let registration = tokio::spawn({
            let producer = live.clone();
            let request = request.clone();
            async move { producer.register(request).await }
        });

        commit_reached.wait().await;
        assert_eq!(oplog.committed_length(), 1);
        assert!(!registration.is_finished());
        registration.abort();
        registration.await.unwrap_err();
        drop(live);

        let restarted = producer(oplog.clone(), &identity, None).await;
        let handle = restarted.validate_registration(&request).await.unwrap();
        assert_eq!(
            handle.stream_id,
            StreamId::derive(
                identity.environment_id,
                &identity.agent_id,
                identity.fingerprint,
                OplogIndex::INITIAL,
            )
            .unwrap()
        );
        assert_eq!(oplog.committed_length(), 1);
        assert!(restarted.register(request).await.unwrap().replayed);
        assert_eq!(oplog.committed_length(), 1);
    }

    #[test]
    #[timeout("30s")]
    async fn commit_completes_before_backpressured_item_and_terminal_fanout() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live_producer = producer(oplog.clone(), &identity, Some(1)).await;
        let registered = live_producer
            .register(root_registration(&identity))
            .await
            .unwrap();
        let mut reader = live_producer
            .catch_up(registered.value.clone(), None)
            .await
            .unwrap();
        live_producer
            .write_items(
                registered.value.stream_id,
                0,
                StreamItemsPayloadV1::Values(vec![vec![1]]),
            )
            .await
            .unwrap();

        let blocked_item = tokio::spawn({
            let producer = live_producer.clone();
            let stream_id = registered.value.stream_id;
            async move {
                producer
                    .write_items(stream_id, 1, StreamItemsPayloadV1::Values(vec![vec![2]]))
                    .await
            }
        });
        while oplog.committed_length() < 3 {
            tokio::task::yield_now().await;
        }
        assert!(!blocked_item.is_finished());
        assert!(oplog.commit_count() >= 3);
        assert_eq!(reader.next().await.unwrap().unwrap().producer_sequence, 0);
        blocked_item.await.unwrap().unwrap();

        let blocked_terminal = tokio::spawn({
            let producer = live_producer.clone();
            let stream_id = registered.value.stream_id;
            async move { producer.end(stream_id, 2, StreamEndResultV1::Ok).await }
        });
        while oplog.committed_length() < 4 {
            tokio::task::yield_now().await;
        }
        assert!(!blocked_terminal.is_finished());
        drop(reader);
        blocked_terminal.await.unwrap().unwrap();

        let restarted = producer(oplog.clone(), &identity, None).await;
        let mut catch_up = restarted.catch_up(registered.value, None).await.unwrap();
        assert_eq!(catch_up.next().await.unwrap().unwrap().producer_sequence, 0);
        assert_eq!(catch_up.next().await.unwrap().unwrap().producer_sequence, 1);
        assert!(matches!(
            catch_up.next().await.unwrap().unwrap().payload,
            CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::Ok)
        ));
    }

    #[test]
    #[timeout("30s")]
    async fn historical_catch_up_does_not_deadlock_with_backpressured_publication() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live_producer = producer(oplog.clone(), &identity, Some(1)).await;
        let handle = live_producer
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value;
        live_producer
            .write_items(
                handle.stream_id,
                0,
                StreamItemsPayloadV1::Values(vec![vec![0]]),
            )
            .await
            .unwrap();

        let bus = live_producer.bus(handle.stream_id).unwrap();
        let mut subscription = bus.subscribe().await.unwrap();
        let join_high_water = subscription.high_water;
        live_producer
            .write_items(
                handle.stream_id,
                1,
                StreamItemsPayloadV1::Values(vec![vec![1]]),
            )
            .await
            .unwrap();
        let blocked_publication = tokio::spawn({
            let producer = live_producer.clone();
            let stream_id = handle.stream_id;
            async move {
                producer
                    .write_items(stream_id, 2, StreamItemsPayloadV1::Values(vec![vec![2]]))
                    .await
            }
        });
        while oplog.committed_length() < 4 {
            tokio::task::yield_now().await;
        }
        assert!(!blocked_publication.is_finished());

        let history = tokio::time::timeout(
            Duration::from_secs(1),
            live_producer.read_segment(&handle, None, join_high_water),
        )
        .await
        .expect("historical catch-up must not wait for bounded live fanout")
        .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].producer_sequence, 0);

        assert_eq!(
            subscription.recv().await.unwrap().payload.producer_sequence,
            1
        );
        blocked_publication.await.unwrap().unwrap();
        assert_eq!(
            subscription.recv().await.unwrap().payload.producer_sequence,
            2
        );
    }

    #[test]
    #[timeout("30s")]
    async fn restart_recovers_an_item_and_terminal_after_commit_before_fanout() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live_producer = producer(oplog.clone(), &identity, Some(1)).await;
        let registered = live_producer
            .register(root_registration(&identity))
            .await
            .unwrap();
        let blocked_reader = live_producer
            .catch_up(registered.value.clone(), None)
            .await
            .unwrap();
        live_producer
            .write_items(
                registered.value.stream_id,
                0,
                StreamItemsPayloadV1::Values(vec![vec![0]]),
            )
            .await
            .unwrap();

        let blocked_item = tokio::spawn({
            let producer = live_producer.clone();
            let stream_id = registered.value.stream_id;
            async move {
                producer
                    .write_items(stream_id, 1, StreamItemsPayloadV1::Values(vec![vec![1]]))
                    .await
            }
        });
        while oplog.committed_length() < 3 {
            tokio::task::yield_now().await;
        }
        assert!(!blocked_item.is_finished());
        blocked_item.abort();
        blocked_item.await.unwrap_err();

        let blocked_terminal = tokio::spawn({
            let producer = live_producer.clone();
            let stream_id = registered.value.stream_id;
            async move { producer.end(stream_id, 2, StreamEndResultV1::Ok).await }
        });
        while oplog.committed_length() < 4 {
            tokio::task::yield_now().await;
        }
        assert!(!blocked_terminal.is_finished());
        blocked_terminal.abort();
        blocked_terminal.await.unwrap_err();
        drop(blocked_reader);
        drop(live_producer);

        let restarted = producer(oplog, &identity, None).await;
        let mut catch_up = restarted.catch_up(registered.value, None).await.unwrap();
        assert_eq!(catch_up.next().await.unwrap().unwrap().producer_sequence, 0);
        assert_eq!(catch_up.next().await.unwrap().unwrap().producer_sequence, 1);
        assert!(matches!(
            catch_up.next().await.unwrap().unwrap().payload,
            CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::Ok)
        ));
    }

    #[test]
    async fn rejected_catch_up_cursor_does_not_consume_live_reader_capacity() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog, &identity, None).await;
        let registered = producer
            .register(root_registration(&identity))
            .await
            .unwrap();
        producer
            .write_items(
                registered.value.stream_id,
                0,
                StreamItemsPayloadV1::Values(vec![vec![1]]),
            )
            .await
            .unwrap();
        let unavailable_cursor = StreamOffsetV1::new(OplogIndex::from_u64(999), 0);

        for _ in 0..golem_common::base_model::durable_stream::MAX_LIVE_READERS_PER_STREAM {
            assert!(matches!(
                producer
                    .catch_up(registered.value.clone(), Some(unavailable_cursor))
                    .await,
                Err(DurableStreamProducerError::CursorUnavailable)
            ));
        }

        producer
            .catch_up(registered.value, None)
            .await
            .expect("rejected admissions must not consume live-reader capacity");
    }

    #[test]
    async fn unavailable_cursor_is_rejected_for_an_empty_stream() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog, &identity, None).await;
        let registered = producer
            .register(root_registration(&identity))
            .await
            .unwrap();
        let unavailable_cursor = StreamOffsetV1::new(OplogIndex::from_u64(999), 0);

        assert!(matches!(
            producer
                .catch_up(registered.value, Some(unavailable_cursor))
                .await,
            Err(DurableStreamProducerError::CursorUnavailable)
        ));
    }

    #[test]
    async fn terminal_delivery_does_not_wait_for_live_reader_cleanup() {
        let bus = Arc::new(DurableLiveStreamBus::new(2).unwrap());
        let subscription = bus.subscribe().await.unwrap();
        let stream_id = StreamId(Uuid::from_u128(99));
        let mut reader = DurableCatchUpReader {
            bus: bus.clone(),
            subscription: Some(subscription),
            history: VecDeque::from([
                CommittedProducerStreamEventV1 {
                    stream_id,
                    producer_sequence: 0,
                    offset: StreamOffsetV1::new(OplogIndex::from_u64(1), 0),
                    packed_u8_batch_end: Some(StreamOffsetV1::new(OplogIndex::from_u64(1), 0)),
                    terminal_author: None,
                    nested_handles: Vec::new(),
                    payload: CommittedProducerStreamEventPayloadV1::PackedU8(7),
                },
                CommittedProducerStreamEventV1 {
                    stream_id,
                    producer_sequence: 1,
                    offset: StreamOffsetV1::new(OplogIndex::from_u64(2), 0),
                    packed_u8_batch_end: None,
                    terminal_author: None,
                    nested_handles: Vec::new(),
                    payload: CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::Ok),
                },
            ]),
            join_high_water: None,
            last_delivered: None,
            terminal_delivered: false,
        };
        assert!(matches!(
            reader.next().await.unwrap().unwrap().payload,
            CommittedProducerStreamEventPayloadV1::PackedU8(7)
        ));

        let (acquired_tx, acquired_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let locked_bus = bus.clone();
        let lock_task = tokio::spawn(async move {
            locked_bus
                .hold_state_lock_until(acquired_tx, release_rx)
                .await;
        });
        acquired_rx.await.unwrap();

        let terminal = tokio::time::timeout(Duration::from_millis(100), reader.next())
            .await
            .expect("terminal delivery waited for live-reader cleanup")
            .unwrap()
            .unwrap();
        assert!(matches!(
            terminal.payload,
            CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::Ok)
        ));
        release_tx.send(()).unwrap();
        lock_task.await.unwrap();
    }

    #[test]
    async fn completed_terminal_catch_up_reader_releases_live_reader_capacity() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog, &identity, None).await;
        let registered = producer
            .register(root_registration(&identity))
            .await
            .unwrap();
        producer
            .end(registered.value.stream_id, 0, StreamEndResultV1::Ok)
            .await
            .unwrap();

        let mut completed = producer
            .catch_up(registered.value.clone(), None)
            .await
            .unwrap();
        assert!(matches!(
            completed.next().await.unwrap().unwrap().payload,
            CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::Ok)
        ));
        assert!(completed.next().await.unwrap().is_none());

        let mut active = Vec::new();
        for _ in 0..golem_common::base_model::durable_stream::MAX_LIVE_READERS_PER_STREAM {
            active.push(
                producer
                    .catch_up(registered.value.clone(), None)
                    .await
                    .expect("a completed terminal reader is no longer a simultaneous live reader"),
            );
        }
    }

    #[test]
    async fn nested_registration_must_match_its_enclosing_stream_coordinate() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog, &identity, None).await;
        let enclosing = producer
            .register(root_registration(&identity))
            .await
            .unwrap();

        let mut other_session = identity.invocation.clone();
        other_session.idempotency_key = IdempotencyKey::new("other-session".to_string());
        let other = producer
            .register(ProducerRegistrationRequestV1 {
                coordinate: StreamRegistrationCoordinateV1::Root {
                    invocation_id: other_session.clone(),
                    root_kind: StreamRootKindV1::MethodResult,
                    recursive_value_path: Vec::new(),
                },
                source_invocation: other_session,
                component_revision: ComponentRevision::INITIAL,
                element_schema_fingerprint: SchemaFingerprintV1([7; 32]),
                source_kind: StreamSourceKindV1::InvocationOutput,
                session_mapping: None,
            })
            .await
            .unwrap();
        let nested_with_wrong_parent = registration(
            &identity,
            StreamRegistrationCoordinateV1::Nested {
                parent_stream_id: other.value.stream_id,
                parent_producer_sequence: 0,
                recursive_value_path: vec![StreamValuePathStepV1::OptionSome],
            },
            StreamSourceKindV1::Nested,
        );

        assert_eq!(
            producer
                .write_items_with_nested(
                    enclosing.value.stream_id,
                    0,
                    StreamItemsPayloadV1::Values(vec![vec![1]]),
                    vec![nested_with_wrong_parent],
                )
                .await,
            Err(DurableStreamProducerError::RegistrationDivergence)
        );
    }

    #[test]
    async fn session_finish_serializes_with_nested_topology_and_fences_later_events() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live_producer = producer(oplog.clone(), &identity, None).await;
        let root = live_producer
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value;
        let nested = registration(
            &identity,
            StreamRegistrationCoordinateV1::Nested {
                parent_stream_id: root.stream_id,
                parent_producer_sequence: 0,
                recursive_value_path: vec![StreamValuePathStepV1::OptionSome],
            },
            StreamSourceKindV1::Nested,
        );

        let writing = {
            let producer = live_producer.clone();
            tokio::spawn(async move {
                producer
                    .write_items_with_nested(
                        root.stream_id,
                        0,
                        StreamItemsPayloadV1::Values(vec![vec![1]]),
                        vec![nested],
                    )
                    .await
            })
        };
        let finishing = {
            let producer = live_producer.clone();
            let session_key = identity.invocation.clone();
            tokio::spawn(async move {
                producer
                    .finish_session(
                        session_key,
                        Err(b"failed".to_vec()),
                        golem_common::base_model::durable_stream::StreamCancelReasonV1::InvocationFailed,
                    )
                    .await
            })
        };
        let write_result = writing.await.unwrap();
        finishing.await.unwrap().unwrap();
        assert!(
            write_result.is_ok()
                || matches!(
                    &write_result,
                    Err(DurableStreamProducerError::SessionFinished(_))
                )
        );
        assert!(matches!(
            live_producer
                .write_items(
                    root.stream_id,
                    usize::from(write_result.is_ok()) as u64,
                    StreamItemsPayloadV1::Values(vec![vec![2]]),
                )
                .await,
            Err(DurableStreamProducerError::SessionFinished(_))
        ));

        let entries = oplog.entries();
        let OplogEntry::StreamSession {
            record: OplogPayload::Inline(record),
            ..
        } = entries.last().expect("session finish entry is missing")
        else {
            panic!("session finish must be the last committed entry");
        };
        assert!(matches!(
            record.as_ref(),
            StreamSessionRecordV1::Finished(_)
        ));

        drop(live_producer);
        let restarted = producer(oplog, &identity, None).await;
        assert!(matches!(
            restarted
                .write_items(
                    root.stream_id,
                    usize::from(write_result.is_ok()) as u64,
                    StreamItemsPayloadV1::Values(vec![vec![2]]),
                )
                .await,
            Err(DurableStreamProducerError::SessionFinished(_))
        ));
    }
}
