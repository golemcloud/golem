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

pub(crate) mod metadata;

use crate::durable_host::stream_bus::{
    DurableLiveStreamBus, DurableLiveStreamBusError, DurableLiveStreamEvent,
    DurableLiveStreamSubscription, PublicationReceipt, QueuedDurableEvent,
};
use crate::services::activity::{ActivityGate, spawn_with_activity};
#[cfg(test)]
use crate::services::oplog::CommitLevel;
use crate::services::oplog::{
    DurableStreamOplogRecord, Oplog, OplogOps, OplogService, OplogServiceOps,
};
use crate::services::rpc::{DurableStreamReadError, Rpc};
use crate::services::worker::WorkerService;
use async_trait::async_trait;
use futures::FutureExt;
use golem_common::base_model::component::ComponentRevision;
use golem_common::base_model::durable_stream::{
    AttachedStreamSegmentRequestV1, AttachmentId, DEFAULT_LIVE_JOIN_BUFFER_SIZE,
    DURABLE_STREAM_FORMAT_VERSION, DurableStreamHandleV1, ExternalProducerIdV1,
    InputStreamHighWaterV1, MAX_DURABLE_STREAM_ITEM_SIZE, MAX_DURABLE_STREAMS_PER_SESSION,
    MAX_NEW_STREAM_HANDLES_PER_VALUE, MAX_PACKED_U8_STREAM_ITEM_SIZE,
    MAX_STREAM_VALUE_TRAVERSAL_DEPTH, STREAM_ATTACHMENT_LEASE_TTL_MILLIS, SessionStreamRoleV1,
    StreamAttachmentActivatedRecordV1, StreamAttachmentControlOperationV1,
    StreamAttachmentControlRequestV1, StreamAttachmentFinalizationReasonV1,
    StreamAttachmentFinalizedRecordV1, StreamAttachmentKeyV1, StreamAttachmentPreparedRecordV1,
    StreamAttachmentRenewedRecordV1, StreamCancelReasonV1, StreamCancelRecordV1,
    StreamCancelRoleV1, StreamCascadeDependentResultV1, StreamCascadeOutboxRecordV1,
    StreamEndRecordV1, StreamEndResultV1, StreamExternalProducerStateRecordV1, StreamId,
    StreamInvocationIdV1, StreamItemsPayloadV1, StreamItemsRecordV1, StreamOffsetV1,
    StreamProducerDeletingRecordV1, StreamRegisteredRecordV1, StreamRegistrationCoordinateV1,
    StreamSessionAttachedRecordV1, StreamSessionFinishedRecordV1,
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
use metadata::{ProducerMetadataKey, ProducerMetadataRow};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use tokio::sync::{Mutex, MutexGuard, Notify, OwnedSemaphorePermit, Semaphore, oneshot};
use tokio_util::sync::CancellationToken;

tokio::task_local! {
    static MUTATION_SCOPE: Arc<ProducerMutationScope>;
    static MUTATION_EFFECTS: ProducerMutationEffects;
}

struct ProducerMutationScope {
    producer: Arc<DurableStreamProducer>,
    commit_tails: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>,
    session_records_changed: AtomicBool,
    publications: std::sync::Mutex<Vec<PublicationReceipt>>,
    remote_cancellations: std::sync::Mutex<
        Vec<futures::future::BoxFuture<'static, Result<(), DurableStreamProducerError>>>,
    >,
    _operation: OwnedSemaphorePermit,
    _memory: OwnedSemaphorePermit,
}

struct ProducerMutationEffects {
    producer: Arc<DurableStreamProducer>,
    pending: AtomicBool,
}

impl Drop for ProducerMutationEffects {
    fn drop(&mut self) {
        if self.pending.load(Ordering::Acquire) {
            self.producer.poison();
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProducerRegistrationRequestV1 {
    pub(crate) coordinate: StreamRegistrationCoordinateV1,
    pub(crate) source_invocation: StreamInvocationIdV1,
    pub(crate) entity_parent_start_index: Option<OplogIndex>,
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

pub(crate) enum ProducerOutputSourceV1 {
    New(ProducerRegistrationRequestV1),
    Existing(DurableStreamHandleV1),
}

pub(crate) struct ProducerOutputRegistrationV1 {
    pub(crate) transport_stream_id: u64,
    pub(crate) source: ProducerOutputSourceV1,
    pub(crate) cancellation_epoch: Option<u64>,
}

pub(crate) struct PendingCommittedCancellation {
    stream_id: StreamId,
    sequence: u64,
    role: StreamCancelRoleV1,
    reason: StreamCancelReasonV1,
    publication: PublicationReceipt,
    event: CommittedProducerStreamEventV1,
    outcome: Result<ProducerWriteOutcomeV1<StreamOffsetV1>, DurableStreamProducerError>,
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

#[derive(Clone, Debug, Eq, PartialEq, desert_rust::BinaryCodec)]
pub(crate) struct StreamHandleReadResultV1 {
    pub(crate) events: Vec<CommittedProducerStreamEventV1>,
    pub(crate) next_offset: Option<StreamOffsetV1>,
    pub(crate) head_offset: Option<StreamOffsetV1>,
    pub(crate) closed: bool,
    pub(crate) cancelled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExternalProducerV1 {
    pub(crate) id: ExternalProducerIdV1,
    pub(crate) epoch: u64,
    pub(crate) sequence: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ExternalAppendOutcomeV1 {
    Accepted(StreamOffsetV1),
    Duplicate {
        offset: StreamOffsetV1,
        highest_sequence: Option<u64>,
    },
    EpochFenced(u64),
    SeqGap {
        expected: u64,
        received: u64,
    },
    Closed,
    NotFound,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DurableStreamProducerError {
    UnsupportedVersion(u8),
    InvalidHandle,
    RegistrationDivergence,
    UnknownStream(StreamId),
    AlreadyTerminal(StreamId),
    FencedByTerminal(CommittedProducerStreamEventPayloadV1),
    ClosedByOtherProducer,
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
    RecoveryRequired,
    LiveBus(DurableLiveStreamBusError),
}

impl std::fmt::Display for DurableStreamProducerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for DurableStreamProducerError {}

impl From<DurableStreamProducerError> for String {
    fn from(error: DurableStreamProducerError) -> Self {
        error.to_string()
    }
}

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
        match value {
            DurableLiveStreamBusError::Retired => Self::RecoveryRequired,
            value => Self::LiveBus(value),
        }
    }
}

#[derive(Clone, Default)]
struct ProducerStreamIndex {
    loaded_metadata: HashSet<metadata::ProducerMetadataKey>,
    registrations: HashMap<StreamId, StreamRegisteredRecordV1>,
    entity_parent_start_indices: HashMap<StreamId, Option<OplogIndex>>,
    referenced_handles: HashMap<StreamId, (DurableStreamHandleV1, HashSet<StreamSessionKeyV1>)>,
    coordinates: HashMap<StreamRegistrationCoordinateV1, StreamId>,
    streams: HashMap<StreamId, IndexedProducerStream>,
    stream_sessions: HashMap<StreamId, StreamSessionKeyV1>,
    stream_roles: HashMap<StreamId, SessionStreamRoleV1>,
    session_stream_mappings:
        HashMap<StreamSessionKeyV1, HashSet<(DurableStreamHandleV1, SessionStreamRoleV1)>>,
    session_stream_counts: HashMap<StreamSessionKeyV1, usize>,
    session_entity_parent_start_indices: HashMap<StreamSessionKeyV1, Option<OplogIndex>>,
    open_session_streams: HashMap<StreamSessionKeyV1, HashSet<StreamId>>,
    invocation_results: HashMap<StreamSessionKeyV1, OplogIndex>,
    finished_sessions: HashSet<StreamSessionKeyV1>,
    attachments: HashMap<(AttachmentId, StreamId, EnvironmentId, AgentId), IndexedStreamAttachment>,
    active_attachments_by_session_stream: HashMap<(StreamSessionKeyV1, StreamId), u64>,
    active_attachment_count: u64,
    attachment_pages: HashMap<u64, Vec<(AttachmentId, StreamId, EnvironmentId, AgentId)>>,
    attachment_positions: HashMap<(AttachmentId, StreamId, EnvironmentId, AgentId), Option<u64>>,
    batch_positions: HashMap<(StreamId, OplogIndex), (u64, u64)>,
    complete_for_deletion: bool,
    cascade_outbox: HashMap<StreamAttachmentKeyV1, StreamCascadeDependentResultV1>,
    consumer_journals: HashMap<(StreamSessionKeyV1, StreamId), IndexedConsumerJournal>,
    external_producer_heads:
        HashMap<(StreamSessionKeyV1, StreamId, ExternalProducerIdV1), IndexedExternalProducer>,
    external_producer_offsets:
        HashMap<(StreamSessionKeyV1, StreamId, ExternalProducerIdV1, u64, u64), StreamOffsetV1>,
    open_streams: usize,
    deleting: bool,
    consumer_deleting: bool,
}

#[derive(Clone, Default, desert_rust::BinaryCodec)]
pub struct IndexedConsumerJournal {
    next_read_ordinal: u64,
    terminal: bool,
    last_source_offset: Option<StreamOffsetV1>,
    source_unavailable: Option<(StreamAttachmentKeyV1, StreamOffsetV1)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProducerJournalSummary {
    event_count: u64,
    last_offset: Option<StreamOffsetV1>,
    terminal: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ConsumerJournalSummary {
    event_count: u64,
    last_offset: Option<StreamOffsetV1>,
    terminal: bool,
    source_unavailable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, desert_rust::BinaryCodec)]
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

#[derive(Clone, Debug, Eq, PartialEq, desert_rust::BinaryCodec)]
pub struct IndexedStreamAttachment {
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
    terminal_event: Option<CommittedProducerStreamEventV1>,
    batches: BTreeMap<u64, OplogIndex>,
    first_sequence: Option<u64>,
    next_sequence: u64,
    last_offset: Option<StreamOffsetV1>,
    terminal: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, desert_rust::BinaryCodec)]
pub struct IndexedExternalProducer {
    epoch: u64,
    last_sequence: u64,
    next_sequence: u64,
    last_offset: StreamOffsetV1,
}

impl IndexedProducerStream {
    fn offsets(&self) -> Vec<StreamOffsetV1> {
        let mut offsets = Vec::new();
        let mut batches = self.batches.iter().peekable();
        while let Some((&first_sequence, &index)) = batches.next() {
            let end = batches
                .peek()
                .map_or(self.next_sequence, |(sequence, _)| **sequence);
            offsets.extend(
                (0..end - first_sequence)
                    .map(|sub_index| StreamOffsetV1::new(index, sub_index as u32)),
            );
        }
        if self.terminal {
            offsets.extend(self.last_offset);
        }
        offsets
    }
}

impl ProducerStreamIndex {
    fn apply_external_producer_state(&mut self, record: &StreamExternalProducerStateRecordV1) {
        let base = (
            record.session_key.clone(),
            record.stream_id,
            record.producer_id.clone(),
        );
        self.external_producer_heads.insert(
            base.clone(),
            IndexedExternalProducer {
                epoch: record.epoch,
                last_sequence: record.sequence,
                next_sequence: record.next_sequence,
                last_offset: record.resulting_offset,
            },
        );
        self.external_producer_offsets.insert(
            (base.0, base.1, base.2, record.epoch, record.sequence),
            record.resulting_offset,
        );
    }

    fn entity_parent_start_index(
        &self,
        stream_id: StreamId,
    ) -> Result<Option<OplogIndex>, DurableStreamProducerError> {
        self.entity_parent_start_indices
            .get(&stream_id)
            .copied()
            .ok_or(DurableStreamProducerError::UnknownStream(stream_id))
    }

    fn session_entity_parent_start_index(
        &self,
        session_key: &StreamSessionKeyV1,
    ) -> Option<OplogIndex> {
        self.session_entity_parent_start_indices
            .get(session_key)
            .copied()
            .flatten()
    }

    fn apply_session_attribution(
        &mut self,
        session_key: &StreamSessionKeyV1,
        entity_parent_start_index: Option<OplogIndex>,
    ) -> Result<(), DurableStreamProducerError> {
        match self.session_entity_parent_start_indices.get(session_key) {
            Some(existing) if *existing != entity_parent_start_index => {
                Err(DurableStreamProducerError::CorruptHistory(
                    "durable stream session contains conflicting entity attribution".to_string(),
                ))
            }
            Some(_) => Ok(()),
            None => {
                self.session_entity_parent_start_indices
                    .insert(session_key.clone(), entity_parent_start_index);
                Ok(())
            }
        }
    }

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

    fn apply_result_offset(&mut self, index: OplogIndex, record: &StreamSessionRecordV1) {
        if let StreamSessionRecordV1::InvocationResult(record) = record {
            self.invocation_results
                .entry(record.session_key.clone())
                .or_insert(index);
        }
    }

    fn apply_session_references(
        &mut self,
        entity_parent_start_index: Option<OplogIndex>,
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
        if let Some(session_key) = crate::worker::stream_session_record_key(record) {
            self.apply_session_attribution(session_key, entity_parent_start_index)?;
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
        if matches!(
            record,
            StreamSessionRecordV1::ConsumerItemValue(_)
                | StreamSessionRecordV1::ConsumerTerminal(_)
                | StreamSessionRecordV1::SourceUnavailable(_)
        ) && !record.has_supported_format()
        {
            return Err(DurableStreamProducerError::CorruptHistory(
                "unsupported or malformed durable consumer journal record".to_string(),
            ));
        }
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
                    .get(&(record.key.session_key.clone(), record.key.stream_id));
                if let Some(journal) = journal
                    && let Some((existing_key, existing_offset)) = &journal.source_unavailable
                {
                    return if existing_key == &record.key
                        && existing_offset == &record.source_offset
                        && record.consumer_read_ordinal == journal.next_read_ordinal
                    {
                        Ok(())
                    } else {
                        Err(DurableStreamProducerError::AttachmentConflict)
                    };
                }
                if journal.is_some_and(|journal| journal.terminal)
                    || record.consumer_read_ordinal
                        != journal.map_or(0, |journal| journal.next_read_ordinal)
                {
                    return Err(DurableStreamProducerError::ConsumerJournalAdvanced);
                }
                let journal = self
                    .consumer_journals
                    .entry((record.key.session_key.clone(), record.key.stream_id))
                    .or_default();
                journal.source_unavailable = Some((record.key.clone(), record.source_offset));
                return Ok(());
            }
            _ => return Ok(()),
        };
        let key = (session_key.clone(), stream_id);
        let journal = self.consumer_journals.get(&key);
        if journal.is_some_and(|journal| journal.terminal || journal.source_unavailable.is_some())
            || ordinal != journal.map_or(0, |journal| journal.next_read_ordinal)
        {
            return Err(DurableStreamProducerError::ConsumerJournalAdvanced);
        }
        let next_read_ordinal = ordinal
            .checked_add(
                u64::try_from(item_count)
                    .map_err(|_| DurableStreamProducerError::CounterOverflow)?,
            )
            .ok_or(DurableStreamProducerError::CounterOverflow)?;
        let last_source_offset = match record {
            StreamSessionRecordV1::ConsumerItemValue(record) => record
                .logical_item_count()
                .checked_sub(1)
                .and_then(|index| record.source_offset_at(index))
                .ok_or_else(|| {
                    DurableStreamProducerError::CorruptHistory(
                        "consumer journal source offset range is empty or invalid".to_string(),
                    )
                })?,
            StreamSessionRecordV1::ConsumerTerminal(record) => record.source_offset,
            _ => unreachable!(),
        };
        let journal = self.consumer_journals.entry(key).or_default();
        journal.next_read_ordinal = next_read_ordinal;
        journal.last_source_offset = Some(last_source_offset);
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
        entity_parent_start_index: Option<OplogIndex>,
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
            let existing_entity_parent_start_index = self
                .entity_parent_start_indices
                .get(existing_id)
                .copied()
                .flatten();
            return if existing == &record
                && existing_entity_parent_start_index == entity_parent_start_index
            {
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
        if let StreamRegistrationCoordinateV1::Nested {
            parent_stream_id, ..
        } = &record.coordinate
            && self
                .entity_parent_start_indices
                .get(parent_stream_id)
                .copied()
                .flatten()
                != entity_parent_start_index
        {
            return Err(DurableStreamProducerError::CorruptHistory(
                "nested stream attribution differs from its parent stream".to_string(),
            ));
        }
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
        self.apply_session_attribution(&session_key, entity_parent_start_index)?;
        self.coordinates
            .insert(record.coordinate.clone(), record.handle.stream_id);
        self.entity_parent_start_indices
            .insert(record.handle.stream_id, entity_parent_start_index);
        self.streams
            .insert(record.handle.stream_id, IndexedProducerStream::default());
        self.stream_sessions
            .insert(record.handle.stream_id, session_key.clone());
        self.stream_roles.insert(record.handle.stream_id, role);
        self.open_session_streams
            .entry(session_key.clone())
            .or_default()
            .insert(record.handle.stream_id);
        if new_session_mapping {
            self.session_stream_mappings
                .entry(session_key.clone())
                .or_default()
                .insert(mapping_identity);
            *self.session_stream_counts.entry(session_key).or_default() += 1;
        }
        self.registrations.insert(record.handle.stream_id, record);
        self.open_streams += 1;
        Ok(())
    }

    fn apply_item_batch(
        &mut self,
        oplog_index: OplogIndex,
        entity_parent_start_index: Option<OplogIndex>,
        pending_registrations: Vec<(OplogIndex, Option<OplogIndex>, StreamRegisteredRecordV1)>,
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
        if pending_registrations.is_empty() {
            return self.apply_items(
                oplog_index,
                entity_parent_start_index,
                record,
                producer_fingerprint,
            );
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
        for (position, ((registration_index, _, registration), expected_stream_id)) in
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
        for (registration_index, entity_parent_start_index, registration) in pending_registrations {
            updated.apply_registration(
                registration_index,
                entity_parent_start_index,
                registration,
                environment_id,
                producer,
                producer_fingerprint,
            )?;
        }
        let events = updated.apply_items(
            oplog_index,
            entity_parent_start_index,
            record,
            producer_fingerprint,
        )?;
        *self = updated;
        Ok(events)
    }

    fn apply_items(
        &mut self,
        oplog_index: OplogIndex,
        entity_parent_start_index: Option<OplogIndex>,
        record: StreamItemsRecordV1,
        producer_fingerprint: AgentFingerprint,
    ) -> Result<Vec<CommittedProducerStreamEventV1>, DurableStreamProducerError> {
        validate_version(record.format_version)?;
        if record.producer_fingerprint != producer_fingerprint {
            return Err(DurableStreamProducerError::InvalidHandle);
        }
        if self.entity_parent_start_index(record.stream_id)? != entity_parent_start_index {
            return Err(DurableStreamProducerError::CorruptHistory(
                "stream item attribution differs from its registration".to_string(),
            ));
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
            events.push(event);
        }
        stream.first_sequence.get_or_insert(record.first_sequence);
        stream.next_sequence = stream
            .next_sequence
            .checked_add(events.len() as u64)
            .ok_or(DurableStreamProducerError::CounterOverflow)?;
        stream.last_offset = record.offsets.last().copied();
        stream.batches.insert(record.first_sequence, oplog_index);
        self.batch_positions.insert(
            (record.stream_id, oplog_index),
            (record.first_sequence, events.len() as u64),
        );
        Ok(events)
    }

    fn apply_end(
        &mut self,
        oplog_index: OplogIndex,
        entity_parent_start_index: Option<OplogIndex>,
        record: StreamEndRecordV1,
        producer_fingerprint: AgentFingerprint,
    ) -> Result<CommittedProducerStreamEventV1, DurableStreamProducerError> {
        validate_version(record.format_version)?;
        if record.producer_fingerprint != producer_fingerprint
            || record.offset != StreamOffsetV1::new(oplog_index, 0)
        {
            return Err(DurableStreamProducerError::InvalidHandle);
        }
        if self.entity_parent_start_index(record.stream_id)? != entity_parent_start_index {
            return Err(DurableStreamProducerError::CorruptHistory(
                "stream end attribution differs from its registration".to_string(),
            ));
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
        stream.first_sequence.get_or_insert(record.sequence);
        stream.last_offset = Some(record.offset);
        stream.terminal = true;
        self.open_streams -= 1;
        if let Some(session) = self.stream_sessions.get(&record.stream_id)
            && let Some(streams) = self.open_session_streams.get_mut(session)
        {
            streams.remove(&record.stream_id);
        }
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
        if self
            .open_session_streams
            .get(&record.session_key)
            .is_some_and(|streams| !streams.is_empty())
        {
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
        entity_parent_start_index: Option<OplogIndex>,
        record: StreamCancelRecordV1,
        producer_fingerprint: AgentFingerprint,
    ) -> Result<CommittedProducerStreamEventV1, DurableStreamProducerError> {
        validate_version(record.format_version)?;
        if record.producer_fingerprint != producer_fingerprint
            || record.offset != StreamOffsetV1::new(oplog_index, 0)
        {
            return Err(DurableStreamProducerError::InvalidHandle);
        }
        if self.entity_parent_start_index(record.stream_id)? != entity_parent_start_index {
            return Err(DurableStreamProducerError::CorruptHistory(
                "stream cancellation attribution differs from its registration".to_string(),
            ));
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
        stream.first_sequence.get_or_insert(record.sequence);
        stream.last_offset = Some(record.offset);
        stream.terminal = true;
        self.open_streams -= 1;
        if let Some(session) = self.stream_sessions.get(&record.stream_id)
            && let Some(streams) = self.open_session_streams.get_mut(session)
        {
            streams.remove(&record.stream_id);
        }
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
        let slot = attachment_slot(key);
        let existing = self.attachments.get(&slot);
        let was_active = existing.is_some_and(|attachment| {
            matches!(
                attachment.state,
                IndexedStreamAttachmentState::Active { .. }
            )
        });

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
                        let count = self
                            .active_attachments_by_session_stream
                            .entry((key.session_key.clone(), key.stream_id))
                            .or_default();
                        *count = count.checked_sub(1).ok_or_else(|| {
                            DurableStreamProducerError::CorruptHistory(
                                "active attachment session count underflow".into(),
                            )
                        })?;
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
            let is_active = matches!(state, IndexedStreamAttachmentState::Active { .. });
            if was_active != is_active {
                let count_key = (key.session_key.clone(), key.stream_id);
                let count = self
                    .active_attachments_by_session_stream
                    .entry(count_key)
                    .or_default();
                if is_active {
                    *count = count
                        .checked_add(1)
                        .ok_or(DurableStreamProducerError::CounterOverflow)?;
                } else {
                    *count = count.checked_sub(1).ok_or_else(|| {
                        DurableStreamProducerError::CorruptHistory(
                            "active attachment session count underflow".into(),
                        )
                    })?;
                }
            }
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
        views.sort_by(|left, right| {
            attachment_sort_key(&left.key).cmp(&attachment_sort_key(&right.key))
        });
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
        dependents
            .sort_by(|left, right| attachment_sort_key(left).cmp(&attachment_sort_key(right)));
        dependents
    }

    fn incomplete_cascade_dependents(&self) -> Vec<StreamAttachmentKeyV1> {
        self.live_dependents()
            .into_iter()
            .filter(|key| !self.cascade_outbox.contains_key(key))
            .collect()
    }
}

fn attachment_slot(
    key: &StreamAttachmentKeyV1,
) -> (AttachmentId, StreamId, EnvironmentId, AgentId) {
    (
        key.attachment_id,
        key.stream_id,
        key.consumer_environment_id,
        key.consumer.clone(),
    )
}

fn attachment_sort_key(
    key: &StreamAttachmentKeyV1,
) -> (StreamId, AttachmentId, EnvironmentId, &AgentId, u64) {
    (
        key.stream_id,
        key.attachment_id,
        key.consumer_environment_id,
        &key.consumer,
        key.epoch,
    )
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

fn logical_payloads(
    payload: &StreamItemsPayloadV1,
) -> impl ExactSizeIterator<Item = CommittedProducerStreamEventPayloadV1> + '_ {
    (0..payload.logical_item_count()).map(|index| match payload {
        StreamItemsPayloadV1::Values(values) => {
            CommittedProducerStreamEventPayloadV1::Value(values[index].clone())
        }
        StreamItemsPayloadV1::PackedU8(bytes) => {
            CommittedProducerStreamEventPayloadV1::PackedU8(bytes[index])
        }
    })
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
    self_weak: std::sync::Weak<Self>,
    oplog: Arc<dyn Oplog>,
    commit: DurableStreamCommit,
    control_metadata_provider: std::sync::OnceLock<(Arc<dyn WorkerService>, AgentMode)>,
    environment_id: EnvironmentId,
    producer: AgentId,
    producer_fingerprint: AgentFingerprint,
    index: Mutex<ProducerStreamIndex>,
    poisoned: AtomicBool,
    retirement: CancellationToken,
    durable_activity: Arc<ActivityGate>,
    owned_operations: Arc<Semaphore>,
    owned_operation_bytes: Arc<Semaphore>,
    lifecycle_operations: Arc<Semaphore>,
    lifecycle_operation_bytes: Arc<Semaphore>,
    buses: RwLock<BTreeMap<StreamId, Arc<DurableLiveStreamBus<CommittedProducerStreamEventV1>>>>,
    terminal_dispatcher_started: AtomicBool,
    terminal_progress: Arc<Notify>,
    committed_retention: std::sync::Mutex<CommittedEventRetention>,
    source_cancellations: RwLock<HashMap<StreamId, (u64, CancellationToken)>>,
    next_source_cancellation_id: AtomicU64,
    reconciliation_cursor: AtomicUsize,
    open_stream_count: AtomicUsize,
    live_join_capacity: usize,
    session_records_changed: Notify,
    session_locks:
        std::sync::Mutex<HashMap<StreamSessionKeyV1, std::sync::Weak<tokio::sync::Mutex<()>>>>,
}

// This is a producer-wide budget rather than a per-stream budget. In particular, creating many
// streams cannot multiply the amount of payload retained by a worker.
const COMMITTED_RETENTION_MAX_ENTRIES: usize = 4096;
const COMMITTED_RETENTION_MAX_BYTES: usize = 32 * 1024 * 1024;
const STREAM_SEGMENT_MAX_EVENTS: usize = 256;
const STREAM_SEGMENT_TARGET_BYTES: usize = 24 * 1024 * 1024;

#[derive(Default)]
struct CommittedEventRetention {
    batches: VecDeque<RetainedCommittedBatch>,
    entries: usize,
    bytes: usize,
}

struct RetainedCommittedBatch {
    events: RetainedCommittedEvents,
    encoded_event_bytes: Option<usize>,
    retained_bytes: usize,
}

enum RetainedCommittedEvents {
    Shared(Arc<CommittedProducerStreamEventV1>),
    Packed {
        stream_id: StreamId,
        first_sequence: u64,
        first_offset: StreamOffsetV1,
        bytes: Arc<Vec<u8>>,
        batch_end: StreamOffsetV1,
        nested_handles: Arc<Vec<DurableStreamHandleV1>>,
    },
}

impl RetainedCommittedBatch {
    fn stream_id(&self) -> StreamId {
        match &self.events {
            RetainedCommittedEvents::Shared(event) => event.stream_id,
            RetainedCommittedEvents::Packed { stream_id, .. } => *stream_id,
        }
    }

    fn first_sequence(&self) -> u64 {
        match &self.events {
            RetainedCommittedEvents::Shared(event) => event.producer_sequence,
            RetainedCommittedEvents::Packed { first_sequence, .. } => *first_sequence,
        }
    }

    fn len(&self) -> usize {
        match &self.events {
            RetainedCommittedEvents::Shared(_) => 1,
            RetainedCommittedEvents::Packed { bytes, .. } => bytes.len(),
        }
    }

    fn offset_at(&self, index: usize) -> Option<StreamOffsetV1> {
        match &self.events {
            RetainedCommittedEvents::Shared(event) => (index == 0).then_some(event.offset),
            RetainedCommittedEvents::Packed {
                first_offset,
                bytes,
                ..
            } => {
                if index >= bytes.len() {
                    return None;
                }
                let sub_index = first_offset
                    .sub_index()
                    .checked_add(u32::try_from(index).ok()?)?;
                Some(StreamOffsetV1::new(
                    first_offset.producer_oplog_index(),
                    sub_index,
                ))
            }
        }
    }

    fn index_of(&self, stream_id: StreamId, offset: StreamOffsetV1) -> Option<usize> {
        if self.stream_id() != stream_id {
            return None;
        }
        match &self.events {
            RetainedCommittedEvents::Shared(event) => (event.offset == offset).then_some(0),
            RetainedCommittedEvents::Packed {
                first_offset,
                bytes,
                ..
            } => {
                if offset.producer_oplog_index() != first_offset.producer_oplog_index() {
                    return None;
                }
                let index = offset.sub_index().checked_sub(first_offset.sub_index())? as usize;
                (index < bytes.len()).then_some(index)
            }
        }
    }
}

fn encoded_event_bytes(event: &CommittedProducerStreamEventV1) -> usize {
    golem_common::serialization::serialize(event)
        .expect("committed stream event serialization cannot fail")
        .len()
}

impl DurableStreamProducer {
    pub(crate) fn ensure_healthy(&self) -> Result<(), DurableStreamProducerError> {
        if self.poisoned.load(Ordering::Acquire) {
            Err(DurableStreamProducerError::RecoveryRequired)
        } else {
            Ok(())
        }
    }

    pub(crate) async fn wait_durable_drained(&self) {
        self.durable_activity.wait_drained().await;
    }

    pub(crate) async fn with_metadata_activity<T>(
        &self,
        lookup: impl Future<Output = T>,
    ) -> Result<T, DurableStreamProducerError> {
        self.ensure_healthy()?;
        let activity = self
            .durable_activity
            .inherit_or_enter()
            .ok_or(DurableStreamProducerError::RecoveryRequired)?;
        let result = activity.scope(lookup).await;
        self.ensure_healthy()?;
        Ok(result)
    }

    /// Cancels only a remote waiter; local durable mutations must finish independently.
    pub(crate) async fn remote_until_retired<T>(
        &self,
        remote: impl Future<Output = Result<T, DurableStreamProducerError>>,
    ) -> Result<T, DurableStreamProducerError> {
        tokio::select! {
            biased;
            _ = self.retirement.cancelled() => Err(DurableStreamProducerError::RecoveryRequired),
            result = remote => result,
        }
    }

    pub(crate) fn try_retire_quiescent(&self) -> bool {
        if self.ensure_healthy().is_err() || !self.durable_activity.try_close_if_idle() {
            return false;
        }
        self.poison();
        true
    }

    pub(crate) fn poison(&self) {
        self.poisoned.store(true, Ordering::Release);
        self.retirement.cancel();
        self.durable_activity.close();
        self.owned_operations.close();
        self.owned_operation_bytes.close();
        self.lifecycle_operations.close();
        self.lifecycle_operation_bytes.close();
        for bus in self
            .buses
            .read()
            .expect("durable stream bus map lock poisoned")
            .values()
        {
            bus.retire();
        }
        self.terminal_progress.notify_one();
        self.session_records_changed.notify_waiters();
    }

    fn begin_durable_effect(&self) {
        let _ = MUTATION_EFFECTS.try_with(|scope| {
            if std::ptr::eq(scope.producer.as_ref(), self) {
                scope.pending.store(true, Ordering::Release);
            }
        });
    }

    fn finish_durable_effect(&self) {
        let _ = MUTATION_EFFECTS.try_with(|scope| {
            if std::ptr::eq(scope.producer.as_ref(), self) {
                scope.pending.store(false, Ordering::Release);
            }
        });
    }

    fn track_durable_effects<T, E>(
        self: &Arc<Self>,
        operation: impl Future<Output = Result<T, E>>,
    ) -> impl Future<Output = Result<T, E>> {
        let operation = Box::pin(operation);
        MUTATION_EFFECTS.scope(
            ProducerMutationEffects {
                producer: self.clone(),
                pending: AtomicBool::new(false),
            },
            async move {
                let outcome = operation.await;
                if outcome.is_ok() {
                    self.finish_durable_effect();
                }
                outcome
            },
        )
    }

    pub(crate) async fn run_owned<T, E, F, Fut>(
        &self,
        retained_bytes: usize,
        operation: F,
    ) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<DurableStreamProducerError> + Send + 'static,
        F: FnOnce(Arc<Self>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
    {
        self.run_owned_mutation(retained_bytes, false, operation)
            .await
    }

    pub(crate) async fn run_lifecycle<T, E, F, Fut>(
        &self,
        retained_bytes: usize,
        operation: F,
    ) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<DurableStreamProducerError> + Send + 'static,
        F: FnOnce(Arc<Self>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
    {
        self.run_owned_mutation(retained_bytes, true, operation)
            .await
    }

    pub(crate) fn defer_remote_cancellation(
        &self,
        cancellation: impl Future<Output = Result<(), DurableStreamProducerError>> + Send + 'static,
    ) {
        MUTATION_SCOPE.with(|scope| {
            assert!(std::ptr::eq(scope.producer.as_ref(), self));
            scope
                .remote_cancellations
                .lock()
                .expect("remote cancellation list lock poisoned")
                .push(Box::pin(cancellation));
        });
    }

    async fn run_owned_mutation<T, E, F, Fut>(
        &self,
        retained_bytes: usize,
        lifecycle: bool,
        operation: F,
    ) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<DurableStreamProducerError> + Send + 'static,
        F: FnOnce(Arc<Self>) -> Fut + Send + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
    {
        self.ensure_healthy()?;
        let producer = self
            .self_weak
            .upgrade()
            .expect("live producer has an owning Arc");
        if MUTATION_SCOPE
            .try_with(|scope| Arc::ptr_eq(&scope.producer, &producer))
            .unwrap_or(false)
        {
            return producer
                .track_durable_effects(operation(producer.clone()))
                .await;
        }
        if !lifecycle && retained_bytes > 256 * 1024 * 1024 {
            return Err(DurableStreamProducerError::ItemTooLarge.into());
        }
        let (operations, bytes) = if lifecycle {
            (&self.lifecycle_operations, &self.lifecycle_operation_bytes)
        } else {
            (&self.owned_operations, &self.owned_operation_bytes)
        };
        let permit = operations
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| DurableStreamProducerError::RecoveryRequired)?;
        let memory = bytes
            .clone()
            .acquire_many_owned(
                // A single large lifecycle error must still be finalized. Reserving the
                // entire lane excludes other byte-retaining lifecycle operations.
                retained_bytes.min(256 * 1024 * 1024) as u32,
            )
            .await
            .map_err(|_| DurableStreamProducerError::RecoveryRequired)?;
        self.ensure_healthy()?;
        let activity = self
            .durable_activity
            .try_enter()
            .ok_or(DurableStreamProducerError::RecoveryRequired)?;
        let (reply, result) = oneshot::channel();
        let scope = Arc::new(ProducerMutationScope {
            producer: producer.clone(),
            commit_tails: std::sync::Mutex::new(Vec::new()),
            session_records_changed: AtomicBool::new(false),
            publications: std::sync::Mutex::new(Vec::new()),
            remote_cancellations: std::sync::Mutex::new(Vec::new()),
            _operation: permit,
            _memory: memory,
        });
        tokio::spawn(async move {
            MUTATION_SCOPE
                .scope(scope.clone(), async move {
                    let mut outcome = match std::panic::AssertUnwindSafe(async {
                        activity
                            .clone()
                            .scope(producer.track_durable_effects(operation(producer.clone())))
                            .await
                    })
                    .catch_unwind()
                    .await
                    {
                        Ok(outcome) => outcome,
                        Err(_) => {
                            producer.poison();
                            Err(DurableStreamProducerError::Oplog(
                                "durable stream mutation panicked".into(),
                            )
                            .into())
                        }
                    };
                    let mut publications = std::mem::take(
                        &mut *scope
                            .publications
                            .lock()
                            .expect("publication receipt list lock poisoned"),
                    );
                    let tails = std::mem::take(
                        &mut *scope
                            .commit_tails
                            .lock()
                            .expect("commit tail list lock poisoned"),
                    );
                    for tail in tails {
                        if let Err(error) = tail.await {
                            producer.poison();
                            outcome = Err(DurableStreamProducerError::Oplog(format!(
                                "durable stream commit callback failed: {error}"
                            ))
                            .into());
                        }
                    }
                    // Slot readers use published worker status, which is folded after the
                    // durability receipt but before the commit callback completes.
                    if scope.session_records_changed.load(Ordering::Acquire) {
                        producer.session_records_changed.notify_waiters();
                    }
                    // Storage quiescence excludes live fanout. The separate count/byte
                    // reservations still bound normal publications until delivery completes.
                    drop(activity);
                    let cancellations = std::mem::take(
                        &mut *scope
                            .remote_cancellations
                            .lock()
                            .expect("remote cancellation list lock poisoned"),
                    );
                    if outcome.is_ok() {
                        for cancellation in cancellations {
                            // A routed call must not inherit local mutation admission or
                            // durable activity, even when it routes back to this executor.
                            match tokio::spawn(cancellation).await {
                                Ok(Ok(())) => {}
                                Ok(Err(error)) => outcome = Err(error.into()),
                                Err(error) => {
                                    outcome = Err(DurableStreamProducerError::Oplog(format!(
                                        "remote stream cancellation failed: {error}"
                                    ))
                                    .into());
                                }
                            }
                        }
                    }
                    if !lifecycle {
                        for publication in publications.drain(..) {
                            if let Err(error) = publication
                                .await
                                .unwrap_or(Err(DurableLiveStreamBusError::PublicationAborted))
                            {
                                producer.poison();
                                outcome = Err(DurableStreamProducerError::from(error).into());
                            }
                        }
                    }
                    let _ = reply.send((outcome, publications));
                })
                .await;
        });
        let (mut outcome, publications) = result
            .await
            .expect("durable stream producer-owned operation terminated");
        // Terminal delivery is owned by the shared dispatcher. The request still observes
        // backpressure, but neither a stalled nor an abandoned request holds lifecycle admission.
        for publication in publications {
            if let Err(error) = publication
                .await
                .unwrap_or(Err(DurableLiveStreamBusError::PublicationAborted))
            {
                self.poison();
                outcome = Err(DurableStreamProducerError::from(error).into());
            }
        }
        outcome
    }

    pub(crate) fn set_control_metadata_provider(
        &self,
        service: Arc<dyn WorkerService>,
        mode: AgentMode,
    ) {
        assert!(self.control_metadata_provider.set((service, mode)).is_ok());
    }

    pub(crate) async fn persisted_control_metadata(
        &self,
        key: &StreamSessionKeyV1,
    ) -> Result<Option<super::durable_session::SessionControlMetadata>, String> {
        self.ensure_healthy()?;
        let Some((service, mode)) = self.control_metadata_provider.get() else {
            return Ok(None);
        };
        let activity = self
            .durable_activity
            .inherit_or_enter()
            .ok_or(DurableStreamProducerError::RecoveryRequired)?;
        let owner = OwnedAgentId::new(self.environment_id, &self.producer);
        activity
            .scope(service.lookup_durable_stream_control_metadata(&owner, *mode, key))
            .await
            .map(Some)
    }

    pub(crate) async fn persisted_consumer_positions(
        &self,
        key: &StreamSessionKeyV1,
        stream: StreamId,
    ) -> Result<Option<(OplogIndex, Vec<OplogIndex>)>, String> {
        self.ensure_healthy()?;
        let Some((service, mode)) = self.control_metadata_provider.get() else {
            return Ok(None);
        };
        let activity = self
            .durable_activity
            .inherit_or_enter()
            .ok_or(DurableStreamProducerError::RecoveryRequired)?;
        activity
            .scope(async {
                let owner = OwnedAgentId::new(self.environment_id, &self.producer);
                let metadata = service
                    .lookup_durable_stream_control_metadata(&owner, *mode, key)
                    .await?;
                let count = metadata
                    .consumer_record_counts
                    .get(&stream)
                    .copied()
                    .unwrap_or_default();
                let page_size =
                    crate::services::stream_session_index::CONSUMER_JOURNAL_INDEX_PAGE_SIZE;
                let mut positions = Vec::new();
                for page in 0..count.div_ceil(page_size) {
                    let records = service
                        .read_durable_stream_consumer_page(&owner, key, stream, page)
                        .await?;
                    let needed = (count - page * page_size).min(page_size) as usize;
                    if records.len() < needed {
                        return Err(
                            "consumer journal index page is shorter than its coverage".into()
                        );
                    }
                    positions.extend_from_slice(&records[..needed]);
                }
                Ok(Some((metadata.covered_through, positions)))
            })
            .await
    }

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

    #[cfg(test)]
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
        let index = Self::read_complete_index(
            oplog.as_ref(),
            environment_id,
            &producer,
            producer_fingerprint,
        )
        .await?;
        Self::from_index(
            oplog,
            environment_id,
            producer,
            producer_fingerprint,
            live_join_capacity,
            commit,
            index,
        )
    }

    async fn read_complete_index(
        oplog: &dyn Oplog,
        environment_id: EnvironmentId,
        producer: &AgentId,
        producer_fingerprint: AgentFingerprint,
    ) -> Result<ProducerStreamIndex, DurableStreamProducerError> {
        let mut index = ProducerStreamIndex::default();
        let mut pending_nested_registrations = Vec::new();
        let current_index = oplog.current_oplog_index().await;
        let mut covered = OplogIndex::NONE;
        while covered < current_index {
            let count = (current_index.as_u64() - covered.as_u64()).min(1024);
            let entries = oplog.read_exact(covered.next(), count).await;
            for (oplog_index, entry) in entries {
                covered = oplog_index;
                match entry {
                    OplogEntry::StreamRegistered {
                        entity_parent_start_index,
                        record,
                        ..
                    } => {
                        let record = oplog
                            .download_payload(record)
                            .await
                            .map_err(DurableStreamProducerError::Oplog)?;
                        if matches!(
                            &record.coordinate,
                            StreamRegistrationCoordinateV1::Nested { .. }
                        ) {
                            pending_nested_registrations.push((
                                oplog_index,
                                entity_parent_start_index,
                                record,
                            ));
                        } else {
                            if !pending_nested_registrations.is_empty() {
                                return Err(DurableStreamProducerError::CorruptHistory(
                                    "nested registration batch is missing its enclosing item"
                                        .to_string(),
                                ));
                            }
                            index.apply_registration(
                                oplog_index,
                                entity_parent_start_index,
                                record,
                                environment_id,
                                producer,
                                producer_fingerprint,
                            )?;
                        }
                    }
                    OplogEntry::StreamItems {
                        entity_parent_start_index,
                        record,
                        ..
                    } => {
                        let record = oplog
                            .download_payload(record)
                            .await
                            .map_err(DurableStreamProducerError::Oplog)?;
                        index.apply_item_batch(
                            oplog_index,
                            entity_parent_start_index,
                            std::mem::take(&mut pending_nested_registrations),
                            record,
                            environment_id,
                            producer,
                            producer_fingerprint,
                        )?;
                    }
                    OplogEntry::StreamEnd {
                        entity_parent_start_index,
                        record,
                        ..
                    } => {
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
                        index.apply_end(
                            oplog_index,
                            entity_parent_start_index,
                            record,
                            producer_fingerprint,
                        )?;
                    }
                    OplogEntry::StreamCancel {
                        entity_parent_start_index,
                        record,
                        ..
                    } => {
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
                        index.apply_cancel(
                            oplog_index,
                            entity_parent_start_index,
                            record,
                            producer_fingerprint,
                        )?;
                    }
                    OplogEntry::StreamSession {
                        entity_parent_start_index,
                        record,
                        ..
                    } => {
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
                        index.apply_session_references(entity_parent_start_index, &record)?;
                        index.apply_result_offset(oplog_index, &record);
                        if let StreamSessionRecordV1::ExternalProducerState(value) = &record {
                            index.apply_external_producer_state(value);
                        }
                        index.apply_deletion_record(
                            &record,
                            environment_id,
                            producer,
                            producer_fingerprint,
                        )?;
                        index.apply_attachment_record(
                            &record,
                            environment_id,
                            producer,
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

        Ok(index)
    }

    fn from_index(
        oplog: Arc<dyn Oplog>,
        environment_id: EnvironmentId,
        producer: AgentId,
        producer_fingerprint: AgentFingerprint,
        live_join_capacity: usize,
        commit: DurableStreamCommit,
        index: ProducerStreamIndex,
    ) -> Result<Arc<Self>, DurableStreamProducerError> {
        let mut buses = BTreeMap::new();
        for (stream_id, stream) in &index.streams {
            let bus = Arc::new(DurableLiveStreamBus::from_committed_high_water(
                live_join_capacity,
                stream.last_offset,
            )?);
            buses.insert(*stream_id, bus);
        }

        let open_stream_count = index.open_streams;
        crate::metrics::durable_stream::add_open_streams(open_stream_count);
        if !index.streams.is_empty() {
            tracing::debug!(
                recovered_streams = index.streams.len(),
                recovered_open_streams = open_stream_count,
                "Durable stream producer index recovered"
            );
        }
        Ok(Arc::new_cyclic(|self_weak| Self {
            self_weak: self_weak.clone(),
            oplog,
            commit,
            control_metadata_provider: std::sync::OnceLock::new(),
            environment_id,
            producer,
            producer_fingerprint,
            index: Mutex::new(index),
            poisoned: AtomicBool::new(false),
            retirement: CancellationToken::new(),
            durable_activity: ActivityGate::new(),
            owned_operations: Arc::new(Semaphore::new(16)),
            owned_operation_bytes: Arc::new(Semaphore::new(256 * 1024 * 1024)),
            lifecycle_operations: Arc::new(Semaphore::new(16)),
            lifecycle_operation_bytes: Arc::new(Semaphore::new(256 * 1024 * 1024)),
            buses: RwLock::new(buses),
            terminal_dispatcher_started: AtomicBool::new(false),
            terminal_progress: Arc::new(Notify::new()),
            committed_retention: std::sync::Mutex::new(CommittedEventRetention::default()),
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
        self.begin_durable_effect();
        let scope = MUTATION_SCOPE
            .try_with(Arc::clone)
            .ok()
            .filter(|scope| std::ptr::eq(scope.producer.as_ref(), self));
        let Some(scope) = scope else {
            (self.commit)(None).await;
            return;
        };
        let (committed, receipt) = oneshot::channel();
        let callback = (self.commit)(Some(committed));
        let producer = scope.producer.clone();
        let task = spawn_with_activity(async move {
            if let Err(panic) = std::panic::AssertUnwindSafe(callback).catch_unwind().await {
                producer.poison();
                std::panic::resume_unwind(panic);
            }
        });
        scope
            .commit_tails
            .lock()
            .expect("commit tail list lock poisoned")
            .push(task);
        receipt
            .await
            .expect("durable stream commit failed before durability receipt");
    }

    async fn commit_notifying(&self, committed: oneshot::Sender<()>) {
        self.commit().await;
        let _ = committed.send(());
    }

    pub(crate) fn retained_payload_bytes(
        payload: &StreamItemsPayloadV1,
    ) -> Result<usize, DurableStreamProducerError> {
        validate_items_payload(payload)?;
        Ok(match payload {
            StreamItemsPayloadV1::Values(values) => values.iter().map(Vec::len).sum::<usize>() * 2,
            StreamItemsPayloadV1::PackedU8(bytes) => {
                bytes.len() * (std::mem::size_of::<CommittedProducerStreamEventV1>() + 2)
            }
        })
    }

    fn retain_committed_events(&self, events: &[CommittedProducerStreamEventV1]) {
        // Only newly committed writes enter retention; replay repairs publish directly to the bus.
        if events.is_empty() {
            return;
        }
        let mut retained = self
            .committed_retention
            .lock()
            .expect("durable stream committed retention lock poisoned");
        let packed = events.iter().enumerate().all(|(index, event)| {
            let Some(sequence) = events[0].producer_sequence.checked_add(index as u64) else {
                return false;
            };
            let Some(sub_index) = u32::try_from(index)
                .ok()
                .and_then(|index| events[0].offset.sub_index().checked_add(index))
            else {
                return false;
            };
            matches!(
                event.payload,
                CommittedProducerStreamEventPayloadV1::PackedU8(_)
            ) && event.stream_id == events[0].stream_id
                && event.producer_sequence == sequence
                && event.offset
                    == StreamOffsetV1::new(events[0].offset.producer_oplog_index(), sub_index)
                && event.packed_u8_batch_end == events[0].packed_u8_batch_end
                && event.terminal_author.is_none()
                && event.nested_handles == events[0].nested_handles
        }) && events[0].packed_u8_batch_end == events.last().map(|event| event.offset);
        let batches = if packed {
            let bytes: Vec<_> = events
                .iter()
                .map(|event| match event.payload {
                    CommittedProducerStreamEventPayloadV1::PackedU8(byte) => byte,
                    _ => unreachable!(),
                })
                .collect();
            let nested_handles = events[0].nested_handles.clone();
            vec![RetainedCommittedBatch {
                encoded_event_bytes: None,
                retained_bytes: std::mem::size_of::<RetainedCommittedBatch>()
                    .saturating_add(bytes.capacity())
                    .saturating_add(4 * std::mem::size_of::<usize>())
                    .saturating_add(
                        nested_handles
                            .capacity()
                            .saturating_mul(std::mem::size_of::<DurableStreamHandleV1>()),
                    ),
                events: RetainedCommittedEvents::Packed {
                    stream_id: events[0].stream_id,
                    first_sequence: events[0].producer_sequence,
                    first_offset: events[0].offset,
                    bytes: Arc::new(bytes),
                    batch_end: events[0]
                        .packed_u8_batch_end
                        .expect("materialized packed batch has an end offset"),
                    nested_handles: Arc::new(nested_handles),
                },
            }]
        } else {
            // Ordinary values and terminals retain their payload behind a shared allocation.
            events
                .iter()
                .filter_map(|event| {
                    let encoded_event_bytes = encoded_event_bytes(event);
                    let retained_bytes = encoded_event_bytes
                        .saturating_add(std::mem::size_of::<RetainedCommittedBatch>())
                        .saturating_add(std::mem::size_of::<CommittedProducerStreamEventV1>());
                    if retained_bytes > COMMITTED_RETENTION_MAX_BYTES {
                        return None;
                    }
                    Some(RetainedCommittedBatch {
                        encoded_event_bytes: Some(encoded_event_bytes),
                        retained_bytes,
                        events: RetainedCommittedEvents::Shared(Arc::new(event.clone())),
                    })
                })
                .collect()
        };
        for batch in batches {
            retained.entries = retained.entries.saturating_add(1);
            retained.bytes = retained.bytes.saturating_add(batch.retained_bytes);
            retained.batches.push_back(batch);
        }
        while retained.entries > COMMITTED_RETENTION_MAX_ENTRIES
            || retained.bytes > COMMITTED_RETENTION_MAX_BYTES
        {
            let Some(evicted) = retained.batches.pop_front() else {
                break;
            };
            retained.entries = retained.entries.saturating_sub(1);
            retained.bytes = retained.bytes.saturating_sub(evicted.retained_bytes);
        }
    }

    fn retained_segment(
        &self,
        stream_id: StreamId,
        after: Option<StreamOffsetV1>,
        through: Option<StreamOffsetV1>,
    ) -> Option<Vec<CommittedProducerStreamEventV1>> {
        let retained = self
            .committed_retention
            .lock()
            .expect("durable stream committed retention lock poisoned");
        let cursor = after.and_then(|after| {
            retained
                .batches
                .iter()
                .find_map(|batch| batch.index_of(stream_id, after).map(|index| (batch, index)))
        });
        if after.is_some() && cursor.is_none() {
            return None;
        }
        let mut result = Vec::new();
        let mut bytes = 0usize;
        let mut expected_sequence = cursor
            .and_then(|(batch, index)| batch.first_sequence().checked_add(index as u64 + 1))
            .unwrap_or(0);
        let mut found_start = false;
        for batch in &retained.batches {
            if batch.stream_id() != stream_id {
                continue;
            }
            let batch_end_sequence = batch.first_sequence().saturating_add(batch.len() as u64);
            if batch_end_sequence <= expected_sequence {
                continue;
            }
            if batch.first_sequence() > expected_sequence {
                return None;
            }
            found_start = true;
            let start = usize::try_from(expected_sequence - batch.first_sequence()).ok()?;
            match &batch.events {
                RetainedCommittedEvents::Shared(event) => {
                    if through.is_some_and(|through| event.offset > through) {
                        break;
                    }
                    let event_bytes = batch
                        .encoded_event_bytes
                        .expect("ordinary retained event has an encoded size");
                    if !result.is_empty()
                        && bytes.saturating_add(event_bytes) > STREAM_SEGMENT_TARGET_BYTES
                    {
                        break;
                    }
                    bytes = bytes.saturating_add(event_bytes);
                    expected_sequence = expected_sequence.saturating_add(1);
                    result.push((**event).clone());
                }
                RetainedCommittedEvents::Packed {
                    stream_id,
                    first_sequence,
                    bytes: packed_bytes,
                    batch_end,
                    nested_handles,
                    ..
                } => {
                    let available = packed_bytes.len().saturating_sub(start);
                    let mut count = available.min(STREAM_SEGMENT_MAX_EVENTS - result.len());
                    if let Some(through) = through {
                        let through_index = batch.index_of(*stream_id, through);
                        if through < batch.offset_at(start)? {
                            break;
                        }
                        if let Some(through_index) = through_index {
                            count = count.min(through_index.saturating_sub(start) + 1);
                        }
                    }
                    if count == 0 {
                        break;
                    }
                    let representative = CommittedProducerStreamEventV1 {
                        stream_id: *stream_id,
                        producer_sequence: u64::MAX,
                        offset: StreamOffsetV1::new(
                            batch.offset_at(start)?.producer_oplog_index(),
                            u32::MAX,
                        ),
                        packed_u8_batch_end: Some(*batch_end),
                        terminal_author: None,
                        nested_handles: (**nested_handles).clone(),
                        payload: CommittedProducerStreamEventPayloadV1::PackedU8(u8::MAX),
                    };
                    let event_bound = encoded_event_bytes(&representative);
                    if !result.is_empty() || bytes > 0 {
                        count = count.min(
                            STREAM_SEGMENT_TARGET_BYTES
                                .saturating_sub(bytes)
                                .checked_div(event_bound)
                                .unwrap_or(0),
                        );
                    } else if event_bound.saturating_mul(count) > STREAM_SEGMENT_TARGET_BYTES {
                        count = (STREAM_SEGMENT_TARGET_BYTES / event_bound)
                            .max(1)
                            .min(count);
                    }
                    for index in start..start + count {
                        result.push(CommittedProducerStreamEventV1 {
                            stream_id: *stream_id,
                            producer_sequence: first_sequence + index as u64,
                            offset: batch.offset_at(index)?,
                            packed_u8_batch_end: Some(*batch_end),
                            terminal_author: None,
                            nested_handles: (**nested_handles).clone(),
                            payload: CommittedProducerStreamEventPayloadV1::PackedU8(
                                packed_bytes[index],
                            ),
                        });
                    }
                    bytes = bytes.saturating_add(event_bound.saturating_mul(count));
                    expected_sequence = expected_sequence.saturating_add(count as u64);
                }
            }
            if result.last().is_some_and(|event| event.is_terminal())
                || result.len() >= STREAM_SEGMENT_MAX_EVENTS
                || bytes >= STREAM_SEGMENT_TARGET_BYTES
                || result
                    .last()
                    .is_some_and(|event| through == Some(event.offset))
            {
                break;
            }
        }
        if !found_start || result.is_empty() && through.is_none() {
            None
        } else {
            Some(result)
        }
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
        self.append_session_record_attributed(None, record).await
    }

    pub(crate) async fn append_session_record_attributed(
        &self,
        entity_parent_start_index: Option<OplogIndex>,
        record: StreamSessionRecordV1,
    ) -> Result<(), DurableStreamProducerError> {
        let memory = golem_common::serialization::serialize(&record)
            .map_err(DurableStreamProducerError::Oplog)?
            .len();
        self.run_owned(memory, move |owner| async move {
            owner
                .append_session_record_owned(entity_parent_start_index, record)
                .await
        })
        .await
    }

    pub(crate) async fn append_session_record_owned(
        &self,
        entity_parent_start_index: Option<OplogIndex>,
        record: StreamSessionRecordV1,
    ) -> Result<(), DurableStreamProducerError> {
        self.append_session_records_owned(entity_parent_start_index, vec![record])
            .await
    }

    pub(crate) async fn append_session_records_owned(
        &self,
        entity_parent_start_index: Option<OplogIndex>,
        records: Vec<StreamSessionRecordV1>,
    ) -> Result<(), DurableStreamProducerError> {
        if records.iter().any(|record| !record.has_supported_format()) {
            return Err(DurableStreamProducerError::CorruptHistory(
                "unsupported or malformed durable Stream Session record".to_string(),
            ));
        }
        let mut index = self
            .index_for(records.iter().flat_map(ProducerMetadataKey::session_record))
            .await?;
        let mut staged = index.clone();
        for record in &records {
            if staged.deleting
                && !matches!(
                    record,
                    StreamSessionRecordV1::AttachmentFinalized(_)
                        | StreamSessionRecordV1::CascadeOutbox(_)
                        | StreamSessionRecordV1::ConsumerDeleting(_)
                        | StreamSessionRecordV1::SourceUnavailable(_)
                )
            {
                return Err(DurableStreamProducerError::ProducerDeleting);
            }
            if staged.consumer_deleting
                && matches!(
                    record,
                    StreamSessionRecordV1::TopologyPrepared(_)
                        | StreamSessionRecordV1::TopologyActivated(_)
                )
            {
                return Err(DurableStreamProducerError::ConsumerDeleting);
            }
            staged.apply_session_references(entity_parent_start_index, record)?;
            staged.apply_deletion_record(
                record,
                self.environment_id,
                &self.producer,
                self.producer_fingerprint,
            )?;
        }
        let result_keys = records
            .iter()
            .enumerate()
            .filter_map(|(position, record)| match record {
                StreamSessionRecordV1::InvocationResult(record) => {
                    Some((position, record.session_key.clone()))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        self.begin_durable_effect();
        let entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |_| {
                records
                    .into_iter()
                    .map(|record| {
                        DurableStreamOplogRecord::Session(
                            entity_parent_start_index,
                            Box::new(record),
                        )
                    })
                    .collect()
            }))
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        for (position, key) in result_keys {
            staged
                .invocation_results
                .entry(key)
                .or_insert(entries[position].0);
        }
        self.commit().await;
        *index = staged;
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
        self.run_lifecycle(0, move |owner| async move {
            owner
                .commit_source_unavailable_overlay_owned(key, source_offset, consumer_read_ordinal)
                .await
        })
        .await
    }

    async fn commit_source_unavailable_overlay_owned(
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
        let mut index = self
            .index_for([ProducerMetadataKey::ConsumerHead(
                key.session_key.clone(),
                key.stream_id,
            )])
            .await?;
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
        let entity_parent_start_index = index.session_entity_parent_start_index(
            crate::worker::stream_session_record_key(&record)
                .expect("source-unavailable record always identifies a session"),
        );
        index.apply_session_references(entity_parent_start_index, &record)?;
        index.apply_consumer_journal_record(&record)?;
        self.begin_durable_effect();
        self.oplog
            .add(OplogEntry::stream_session(
                entity_parent_start_index,
                OplogPayload::Inline(Box::new(record)),
            ))
            .await;
        self.commit().await;
        self.notify_session_records_changed();
        Ok(false)
    }

    pub(crate) async fn consumer_source_unavailable(
        &self,
        key: &StreamAttachmentKeyV1,
    ) -> Result<Option<StreamOffsetV1>, DurableStreamProducerError> {
        if key.consumer_environment_id != self.environment_id
            || key.consumer != self.producer
            || key.expected_consumer_fingerprint != self.producer_fingerprint
            || key.consumer_invocation.callee_environment_id != self.environment_id
            || key.consumer_invocation.callee != self.producer
            || key.consumer_invocation.callee_fingerprint != self.producer_fingerprint
        {
            return Err(DurableStreamProducerError::InvalidAttachmentState);
        }
        let index = self
            .index_for([ProducerMetadataKey::ConsumerHead(
                key.session_key.clone(),
                key.stream_id,
            )])
            .await?;
        Ok(index
            .consumer_journals
            .get(&(key.session_key.clone(), key.stream_id))
            .and_then(|journal| journal.source_unavailable.as_ref())
            .and_then(|(recorded_key, offset)| (recorded_key == key).then_some(*offset)))
    }

    async fn persist_attachment_record(
        &self,
        record: StreamSessionRecordV1,
    ) -> Result<AttachmentApplyOutcome, DurableStreamProducerError> {
        self.run_lifecycle(0, move |owner| async move {
            owner.persist_attachment_record_owned(record).await
        })
        .await
    }

    async fn persist_attachment_record_owned(
        &self,
        record: StreamSessionRecordV1,
    ) -> Result<AttachmentApplyOutcome, DurableStreamProducerError> {
        if !record.has_supported_format() {
            return Err(DurableStreamProducerError::CorruptHistory(
                "unsupported or malformed durable attachment record".to_string(),
            ));
        }
        let mut index = self
            .index_for(ProducerMetadataKey::session_record(&record))
            .await?;
        let stream_id = match &record {
            StreamSessionRecordV1::AttachmentPrepared(record) => record.key.stream_id,
            StreamSessionRecordV1::AttachmentActivated(record) => record.key.stream_id,
            StreamSessionRecordV1::AttachmentRenewed(record) => record.key.stream_id,
            StreamSessionRecordV1::AttachmentFinalized(record) => record.key.stream_id,
            _ => unreachable!("attachment persistence received a non-attachment record"),
        };
        let entity_parent_start_index = index.entity_parent_start_index(stream_id)?;
        let mut updated = index.clone();
        updated.apply_session_references(entity_parent_start_index, &record)?;
        let outcome = updated.apply_attachment_record(
            &record,
            self.environment_id,
            &self.producer,
            self.producer_fingerprint,
        )?;
        if outcome == AttachmentApplyOutcome::Changed {
            self.begin_durable_effect();
            self.oplog
                .add(OplogEntry::stream_session(
                    entity_parent_start_index,
                    OplogPayload::Inline(Box::new(record)),
                ))
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
        let mut locks = self
            .session_locks
            .lock()
            .expect("durable stream session lock map poisoned");
        if locks.len() >= 128 {
            locks.retain(|_, lock| lock.strong_count() > 0);
        }
        if let Some(lock) = locks.get(session_key).and_then(std::sync::Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        locks.insert(session_key.clone(), Arc::downgrade(&lock));
        lock
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
            .index_for([ProducerMetadataKey::Session(session_key.clone())])
            .await?
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
        let deferred = MUTATION_SCOPE
            .try_with(|scope| {
                if std::ptr::eq(scope.producer.as_ref(), self) {
                    scope.session_records_changed.store(true, Ordering::Release);
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
        if !deferred {
            self.session_records_changed.notify_waiters();
        }
    }

    #[tracing::instrument(name = "durable_stream.register", skip_all)]
    pub(crate) async fn register(
        &self,
        request: ProducerRegistrationRequestV1,
    ) -> Result<ProducerWriteOutcomeV1<DurableStreamHandleV1>, DurableStreamProducerError> {
        self.run_owned(0, move |owner| async move {
            owner.register_owned(request).await
        })
        .await
    }

    async fn register_owned(
        &self,
        request: ProducerRegistrationRequestV1,
    ) -> Result<ProducerWriteOutcomeV1<DurableStreamHandleV1>, DurableStreamProducerError> {
        if registration_coordinate_depth(&request.coordinate) > MAX_STREAM_VALUE_TRAVERSAL_DEPTH {
            crate::metrics::durable_stream::record_limit_violation("traversal_depth");
            return Err(DurableStreamProducerError::TraversalDepthLimit);
        }
        let mut index = self
            .index_for(ProducerMetadataKey::registration(&request))
            .await?;
        if let Some(stream_id) = index.coordinates.get(&request.coordinate) {
            let existing = index
                .registrations
                .get(stream_id)
                .expect("coordinate index points at a missing registration");
            if registration_matches(existing, &request)
                && index.entity_parent_start_index(*stream_id)? == request.entity_parent_start_index
            {
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
        let entity_parent_start_index = request.entity_parent_start_index;
        let request_for_entry = request.clone();
        self.begin_durable_effect();
        let mut entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |oplog_index| {
                vec![DurableStreamOplogRecord::Registered(
                    entity_parent_start_index,
                    registration_record(
                        oplog_index,
                        environment_id,
                        producer,
                        producer_fingerprint,
                        request_for_entry,
                    ),
                )]
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
            entity_parent_start_index,
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
        let memory = golem_common::serialization::serialize(&pending_invocation)
            .map_err(DurableStreamProducerError::Oplog)?
            .len();
        self.run_owned(memory, move |owner| async move {
            owner
                .prepare_session_owned(requests, pending_invocation, committed, make_prepared)
                .await
        })
        .await
    }

    async fn prepare_session_owned(
        &self,
        requests: Vec<(u64, ProducerRegistrationRequestV1)>,
        pending_invocation: OplogEntry,
        committed: oneshot::Sender<()>,
        make_prepared: impl FnOnce(Vec<(u64, DurableStreamHandleV1)>) -> StreamSessionPreparedRecordV1
        + Send
        + 'static,
    ) -> Result<StreamSessionPreparedRecordV1, DurableStreamProducerError> {
        let mut index = self
            .index_for(
                requests
                    .iter()
                    .flat_map(|(_, request)| ProducerMetadataKey::registration(request))
                    .collect::<Vec<_>>(),
            )
            .await?;
        index.ensure_producer_write_allowed()?;
        if requests.len() > MAX_DURABLE_STREAMS_PER_SESSION {
            crate::metrics::durable_stream::record_limit_violation("streams_per_session");
            return Err(DurableStreamProducerError::StreamLimit);
        }
        let entity_parent_start_index = requests
            .first()
            .and_then(|(_, request)| request.entity_parent_start_index);
        for (_, request) in &requests {
            if request.entity_parent_start_index != entity_parent_start_index {
                return Err(DurableStreamProducerError::RegistrationDivergence);
            }
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
        self.begin_durable_effect();
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
                    result.push(DurableStreamOplogRecord::Registered(
                        entity_parent_start_index,
                        record,
                    ));
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
                result.push(DurableStreamOplogRecord::Session(
                    entity_parent_start_index,
                    Box::new(prepared_record),
                ));
                result.push(DurableStreamOplogRecord::InlineEntry(pending_invocation));
                result.push(DurableStreamOplogRecord::Session(
                    entity_parent_start_index,
                    Box::new(StreamSessionRecordV1::Attached(attached)),
                ));
                result
            }))
            .await
            .map_err(DurableStreamProducerError::Oplog)?;

        let mut prepared = None;
        let mut registrations = Vec::with_capacity(requests.len());
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
                        .map_err(DurableStreamProducerError::Oplog)?;
                    registrations.push((oplog_index, entity_parent_start_index, record));
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
        for (oplog_index, entity_parent_start_index, record) in registrations {
            updated_index.apply_registration(
                oplog_index,
                entity_parent_start_index,
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
        updated_index.apply_session_references(
            entity_parent_start_index,
            &StreamSessionRecordV1::Prepared(prepared.clone()),
        )?;

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
        session_key: StreamSessionKeyV1,
        result: Vec<u8>,
        outputs: Vec<ProducerOutputRegistrationV1>,
        entity_parent_start_index: Option<OplogIndex>,
    ) -> Result<(Vec<DurableStreamHandleV1>, StreamSessionRecordV1), DurableStreamProducerError>
    {
        self.run_owned(result.len() * 2, move |owner| async move {
            owner
                .register_result_streams_owned(
                    session_key,
                    result,
                    outputs,
                    entity_parent_start_index,
                )
                .await
        })
        .await
    }

    async fn register_result_streams_owned(
        &self,
        session_key: StreamSessionKeyV1,
        result: Vec<u8>,
        outputs: Vec<ProducerOutputRegistrationV1>,
        entity_parent_start_index: Option<OplogIndex>,
    ) -> Result<(Vec<DurableStreamHandleV1>, StreamSessionRecordV1), DurableStreamProducerError>
    {
        let mut keys = vec![ProducerMetadataKey::Session(session_key.clone())];
        for output in &outputs {
            match &output.source {
                ProducerOutputSourceV1::New(request) => {
                    keys.extend(ProducerMetadataKey::registration(request))
                }
                ProducerOutputSourceV1::Existing(handle) => {
                    keys.push(ProducerMetadataKey::Reference(handle.stream_id));
                    if self.owns_handle_identity(handle) {
                        keys.push(ProducerMetadataKey::Stream(handle.stream_id));
                    }
                }
            }
        }
        let mut index = self.index_for(keys).await?;
        let result_offset = index.invocation_results.get(&session_key).copied();
        let result_session_key = session_key.clone();
        let mut cancellations = Vec::new();
        for (position, output) in outputs.iter().enumerate() {
            if let Some(epoch) = output.cancellation_epoch {
                if epoch == 0 {
                    return Err(DurableStreamProducerError::InvalidAttachmentState);
                }
                let terminal = match &output.source {
                    ProducerOutputSourceV1::New(_) => Some((0, entity_parent_start_index)),
                    ProducerOutputSourceV1::Existing(handle)
                        if self.owns_handle_identity(handle) =>
                    {
                        let stream = index
                            .streams
                            .get(&handle.stream_id)
                            .ok_or(DurableStreamProducerError::UnknownStream(handle.stream_id))?;
                        if stream.terminal {
                            None
                        } else {
                            Some((
                                stream.next_sequence,
                                index.entity_parent_start_index(handle.stream_id)?,
                            ))
                        }
                    }
                    ProducerOutputSourceV1::Existing(_) => None,
                };
                let applied_locally = matches!(&output.source, ProducerOutputSourceV1::New(_))
                    || matches!(&output.source, ProducerOutputSourceV1::Existing(handle) if self.owns_handle_identity(handle));
                cancellations.push((position, epoch, terminal, applied_locally));
            }
        }
        let requests = outputs
            .iter()
            .filter_map(|output| match &output.source {
                ProducerOutputSourceV1::New(request) => Some(request.clone()),
                ProducerOutputSourceV1::Existing(_) => None,
            })
            .collect::<Vec<_>>();
        let make_result = move |owned_handles: Vec<DurableStreamHandleV1>| {
            let mut owned_handles = owned_handles.into_iter();
            let stream_mappings = outputs
                .into_iter()
                .map(|output| {
                    let handle = match output.source {
                        ProducerOutputSourceV1::New(_) => owned_handles
                            .next()
                            .expect("result allocation supplies one handle per new output"),
                        ProducerOutputSourceV1::Existing(handle) => handle,
                    };
                    StreamSessionMappingRecordV1 {
                        transport_stream_id: output.transport_stream_id,
                        handle,
                        role: SessionStreamRoleV1::Output,
                    }
                })
                .collect::<Vec<_>>();
            StreamSessionRecordV1::InvocationResult(
                golem_common::base_model::durable_stream::StreamSessionInvocationResultRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key,
                    result,
                    output_streams: stream_mappings
                        .iter()
                        .map(|mapping| mapping.handle.clone())
                        .collect(),
                    stream_mappings,
                },
            )
        };
        index.ensure_producer_write_allowed()?;
        if requests.len() > MAX_NEW_STREAM_HANDLES_PER_VALUE {
            crate::metrics::durable_stream::record_limit_violation("streams_per_value");
            return Err(DurableStreamProducerError::ValueStreamLimit);
        }
        let mut coordinates = HashSet::new();
        if requests.iter().any(|request| {
            request.entity_parent_start_index != entity_parent_start_index
                || !coordinates.insert(&request.coordinate)
        }) {
            return Err(DurableStreamProducerError::RegistrationDivergence);
        }
        if requests.is_empty()
            && let Some(result_offset) = result_offset
        {
            let expected = make_result(Vec::new());
            let StreamSessionRecordV1::InvocationResult(_) = &expected else {
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
            drop(index);
            let record = self.read_result_record(result_offset).await?;
            return if record == expected {
                Ok((Vec::new(), record))
            } else {
                Err(DurableStreamProducerError::RegistrationDivergence)
            };
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
            if let Some(offset) = result_offset {
                let record = self.read_result_record(offset).await?;
                if record == expected {
                    crate::metrics::durable_stream::record_producer_operation(
                        "register_result",
                        true,
                    );
                    return Ok((handles, record));
                }
            }
            return Err(DurableStreamProducerError::RegistrationDivergence);
        }
        if index.finished_sessions.contains(&result_session_key) {
            return Err(DurableStreamProducerError::SessionFinished(
                result_session_key,
            ));
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
        self.begin_durable_effect();
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
                    result.push(DurableStreamOplogRecord::Registered(
                        entity_parent_start_index,
                        record,
                    ));
                }
                let session_record = make_result(handles);
                if !session_record.has_supported_format() {
                    return Vec::new();
                }
                let StreamSessionRecordV1::InvocationResult(invocation_result) = &session_record else {
                    unreachable!("result builder returns an invocation result");
                };
                let mut cancelled = HashSet::new();
                for (position, epoch, terminal, applied_locally) in cancellations {
                    let handle = &invocation_result.stream_mappings[position].handle;
                    if !cancelled.insert(handle.stream_id) { continue; }
                    result.push(DurableStreamOplogRecord::Session(
                        entity_parent_start_index,
                        Box::new(StreamSessionRecordV1::ConsumerCancelIntent(
                            golem_common::base_model::durable_stream::StreamConsumerCancelIntentRecordV1 {
                                format_version: DURABLE_STREAM_FORMAT_VERSION,
                                session_key: invocation_result.session_key.clone(),
                                stream_id: handle.stream_id,
                                epoch,
                                role: StreamCancelRoleV1::OutputConsumer,
                                reason: StreamCancelReasonV1::Cancelled,
                                details: None,
                            }
                        )),
                    ));
                    if let Some((sequence, attribution)) = terminal {
                        let offset = StreamOffsetV1::new(OplogIndex::from_u64(first_index.as_u64() + result.len() as u64), 0);
                        result.push(DurableStreamOplogRecord::Cancel(attribution, StreamCancelRecordV1 {
                            format_version: DURABLE_STREAM_FORMAT_VERSION,
                            stream_id: handle.stream_id,
                            producer_fingerprint,
                            sequence,
                            offset,
                            authored_by: StreamTerminalAuthorV1::Protocol,
                            role: StreamCancelRoleV1::OutputConsumer,
                            reason: StreamCancelReasonV1::Cancelled,
                            details: None,
                        }));
                    }
                    if applied_locally {
                        result.push(DurableStreamOplogRecord::Session(
                            entity_parent_start_index,
                            Box::new(StreamSessionRecordV1::ConsumerCancelApplied(
                                golem_common::base_model::durable_stream::StreamConsumerCancelAppliedRecordV1 {
                                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                                    intent: golem_common::base_model::durable_stream::StreamConsumerCancelIntentRecordV1 {
                                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                                        session_key: invocation_result.session_key.clone(),
                                        stream_id: handle.stream_id,
                                        epoch,
                                        role: StreamCancelRoleV1::OutputConsumer,
                                        reason: StreamCancelReasonV1::Cancelled,
                                        details: None,
                                    },
                                },
                            )),
                        ));
                    }
                }
                result.push(DurableStreamOplogRecord::Session(
                    entity_parent_start_index,
                    Box::new(session_record),
                ));
                result
            }))
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        self.commit().await;

        let mut handles = Vec::new();
        let mut session_record = None;
        let mut cancelled_count = 0;
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
                        .map_err(DurableStreamProducerError::Oplog)?;
                    handles.push(record.handle.clone());
                    index.apply_registration(
                        oplog_index,
                        entity_parent_start_index,
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
                OplogEntry::StreamSession {
                    entity_parent_start_index,
                    record,
                    ..
                } => {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(DurableStreamProducerError::Oplog)?;
                    index.apply_session_references(entity_parent_start_index, &record)?;
                    index.apply_result_offset(oplog_index, &record);
                    if matches!(record, StreamSessionRecordV1::InvocationResult(_)) {
                        session_record = Some(record);
                    }
                }
                OplogEntry::StreamCancel {
                    entity_parent_start_index,
                    record,
                    ..
                } => {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(DurableStreamProducerError::Oplog)?;
                    let event = index.apply_cancel(
                        oplog_index,
                        entity_parent_start_index,
                        record,
                        self.producer_fingerprint,
                    )?;
                    self.cancel_source(event.stream_id);
                    // The terminal dispatcher publishes without delaying result registration on a reader.
                    drop(self.enqueue_events(event.stream_id, vec![event], false)?);
                    cancelled_count += 1;
                }
                _ => {
                    return Err(DurableStreamProducerError::CorruptHistory(
                        "result registration batch contains an unexpected oplog entry".to_string(),
                    ));
                }
            }
        }
        self.record_registered_streams(handles.len());
        self.record_terminal_streams(cancelled_count);
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
    ) -> Result<Option<DurableStreamHandleV1>, DurableStreamProducerError> {
        let index = self
            .index_for([ProducerMetadataKey::Coordinate(coordinate.clone())])
            .await?;
        Ok(index
            .coordinates
            .get(coordinate)
            .and_then(|stream_id| index.registrations.get(stream_id))
            .map(|registration| registration.handle.clone()))
    }

    pub(crate) async fn stream_head(
        &self,
        handle: &DurableStreamHandleV1,
    ) -> Result<(Option<StreamOffsetV1>, bool, bool), DurableStreamProducerError> {
        let index = self.index_for_terminal([], handle.stream_id).await?;
        if index
            .registrations
            .get(&handle.stream_id)
            .is_none_or(|registration| &registration.handle != handle)
        {
            return Err(DurableStreamProducerError::InvalidHandle);
        }
        let stream = &index.streams[&handle.stream_id];
        let cancelled = stream.terminal_event.as_ref().is_some_and(|event| {
            matches!(
                event.payload,
                CommittedProducerStreamEventPayloadV1::Cancel { .. }
            )
        });
        Ok((stream.last_offset, stream.terminal, cancelled))
    }

    pub(crate) async fn read_by_handle(
        self: &Arc<Self>,
        request: golem_common::model::durable_stream::StreamHandleReadRequestV1,
    ) -> Result<StreamHandleReadResultV1, DurableStreamProducerError> {
        if let Some(offset) = request.after {
            StreamOffsetV1::from_bytes(offset.0)
                .map_err(|error| DurableStreamProducerError::InvalidOffset(error.to_string()))?;
        }
        let (mut head, mut closed, mut cancelled) = self.stream_head(&request.handle).await?;
        if request.max_items == 0 {
            return Ok(StreamHandleReadResultV1 {
                events: Vec::new(),
                next_offset: request.after,
                head_offset: head,
                closed,
                cancelled,
            });
        }
        if request.max_bytes == 0 {
            return Err(DurableStreamProducerError::InvalidValueBatch);
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
                (head, closed, cancelled) = self.stream_head(&request.handle).await?;
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
                CommittedProducerStreamEventPayloadV1::Value(value) => value.len() as u64,
                CommittedProducerStreamEventPayloadV1::PackedU8(_) => 1,
                _ => 0,
            };
            if count == max_items || bytes + size > max_bytes {
                if count == 0 {
                    return Err(DurableStreamProducerError::InvalidValueBatch);
                }
                break;
            }
            bytes += size;
            count += 1;
        }
        events.truncate(count);
        let next_offset = events.last().map(|event| event.offset).or(request.after);
        Ok(StreamHandleReadResultV1 {
            events,
            next_offset,
            head_offset: head,
            closed,
            cancelled,
        })
    }

    pub(crate) async fn validate_registration(
        &self,
        request: &ProducerRegistrationRequestV1,
    ) -> Result<DurableStreamHandleV1, DurableStreamProducerError> {
        let index = self
            .index_for([ProducerMetadataKey::Coordinate(request.coordinate.clone())])
            .await?;
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
        let index = self
            .index_for([ProducerMetadataKey::Session(session_key.clone())])
            .await?;
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
        let index = self
            .index_for([ProducerMetadataKey::Batch(stream_id, first_sequence)])
            .await?;
        let stream = index
            .streams
            .get(&stream_id)
            .ok_or(DurableStreamProducerError::UnknownStream(stream_id))?;
        let oplog_index = *stream
            .batches
            .get(&first_sequence)
            .ok_or(DurableStreamProducerError::EventConflict)?;
        drop(index);
        let record = self.read_item_batch(oplog_index).await?;
        self.resolve_nested_handles(&record.nested_stream_ids).await
    }

    async fn read_result_record(
        &self,
        oplog_index: OplogIndex,
    ) -> Result<StreamSessionRecordV1, DurableStreamProducerError> {
        let OplogEntry::StreamSession { record, .. } = self.oplog.read(oplog_index).await else {
            return Err(DurableStreamProducerError::CorruptHistory(
                "result metadata points at a non-session record".into(),
            ));
        };
        self.oplog
            .download_payload(record)
            .await
            .map_err(DurableStreamProducerError::Oplog)
    }

    async fn read_item_batch(
        &self,
        oplog_index: OplogIndex,
    ) -> Result<StreamItemsRecordV1, DurableStreamProducerError> {
        let OplogEntry::StreamItems { record, .. } = self.oplog.read(oplog_index).await else {
            return Err(DurableStreamProducerError::CorruptHistory(
                "stream batch index points at a non-item record".into(),
            ));
        };
        self.oplog
            .download_payload(record)
            .await
            .map_err(DurableStreamProducerError::Oplog)
    }

    async fn resolve_nested_handles(
        &self,
        stream_ids: &[StreamId],
    ) -> Result<Vec<DurableStreamHandleV1>, DurableStreamProducerError> {
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
                    .ok_or(DurableStreamProducerError::UnknownStream(*stream_id))
            })
            .collect()
    }

    async fn materialize_item_batch(
        &self,
        record: &StreamItemsRecordV1,
    ) -> Result<Vec<CommittedProducerStreamEventV1>, DurableStreamProducerError> {
        self.materialize_item_batch_range(record, record.first_sequence, usize::MAX)
            .await
    }

    async fn materialize_item_batch_range(
        &self,
        record: &StreamItemsRecordV1,
        first_sequence: u64,
        limit: usize,
    ) -> Result<Vec<CommittedProducerStreamEventV1>, DurableStreamProducerError> {
        let nested_handles = self
            .resolve_nested_handles(&record.nested_stream_ids)
            .await?;
        let packed_u8_batch_end = matches!(record.payload, StreamItemsPayloadV1::PackedU8(_))
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
                Ok(CommittedProducerStreamEventV1 {
                    stream_id: record.stream_id,
                    producer_sequence: record
                        .first_sequence
                        .checked_add(sub_index as u64)
                        .ok_or(DurableStreamProducerError::CounterOverflow)?,
                    offset: *offset,
                    packed_u8_batch_end,
                    terminal_author: None,
                    nested_handles: nested_handles.clone(),
                    payload,
                })
            })
            .collect()
    }

    pub(crate) async fn input_high_water(
        &self,
        stream_id: StreamId,
    ) -> Result<Option<InputStreamHighWaterV1>, DurableStreamProducerError> {
        let index = self
            .index_for([ProducerMetadataKey::Stream(stream_id)])
            .await?;
        let stream = index
            .streams
            .get(&stream_id)
            .ok_or(DurableStreamProducerError::UnknownStream(stream_id))?;
        let Some(resulting_offset) = stream.last_offset else {
            return Ok(None);
        };
        Ok(Some(InputStreamHighWaterV1 {
            highest_contiguous_sequence: if stream.terminal {
                stream.next_sequence
            } else {
                stream.next_sequence - 1
            },
            resulting_offset,
            terminal: stream.terminal,
        }))
    }

    pub(crate) async fn attached_input_high_water(
        &self,
        session_key: &StreamSessionKeyV1,
        stream_id: StreamId,
    ) -> Result<Option<InputStreamHighWaterV1>, DurableStreamProducerError> {
        let index = self
            .index_for_terminal(
                [
                    ProducerMetadataKey::Stream(stream_id),
                    ProducerMetadataKey::ExternalProducerHead(
                        session_key.clone(),
                        stream_id,
                        ExternalProducerIdV1::Attached,
                    ),
                ],
                stream_id,
            )
            .await?;
        let stream = index
            .streams
            .get(&stream_id)
            .ok_or(DurableStreamProducerError::UnknownStream(stream_id))?;
        let head = index.external_producer_heads.get(&(
            session_key.clone(),
            stream_id,
            ExternalProducerIdV1::Attached,
        ));
        let next_sequence = head.map_or(0, |head| head.next_sequence);
        if next_sequence == 0 && !stream.terminal {
            return Ok(None);
        }
        let resulting_offset = stream.last_offset.ok_or_else(|| {
            DurableStreamProducerError::CorruptHistory(
                "terminal attached input has no resulting offset".into(),
            )
        })?;
        Ok(Some(InputStreamHighWaterV1 {
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
        let memory = Self::retained_payload_bytes(&payload)?;
        self.run_owned(memory, move |owner| async move {
            owner
                .write_items_owned(
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

    pub(crate) async fn write_attached_items_with_nested(
        self: &Arc<Self>,
        session_key: &StreamSessionKeyV1,
        stream_id: StreamId,
        transport_first_sequence: u64,
        payload: StreamItemsPayloadV1,
        nested: Vec<ProducerRegistrationRequestV1>,
    ) -> Result<ProducerWriteOutcomeV1<Vec<StreamOffsetV1>>, DurableStreamProducerError> {
        let memory = Self::retained_payload_bytes(&payload)?;
        let _transport_next_sequence = transport_first_sequence
            .checked_add(payload.logical_item_count() as u64)
            .ok_or(DurableStreamProducerError::CounterOverflow)?;
        let session_key = session_key.clone();
        self.run_owned(memory, move |owner| async move {
            let id = ExternalProducerIdV1::Attached;
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
                .ok_or(DurableStreamProducerError::UnknownStream(stream_id))?;
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
                                DurableStreamProducerError::CorruptHistory(
                                    "attached producer sequence has no original offset".into(),
                                )
                            })?;
                        if stream.terminal && stream.last_offset == Some(offset) {
                            return Err(DurableStreamProducerError::EventConflict);
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
                                DurableStreamProducerError::ClosedByOtherProducer
                            });
                        }
                        if transport_first_sequence != head.next_sequence {
                            return Err(DurableStreamProducerError::SequenceGap {
                                expected: head.next_sequence,
                                actual: transport_first_sequence,
                            });
                        }
                        (stream.next_sequence, Some(index))
                    }
                } else {
                    if stream.terminal {
                        return Err(DurableStreamProducerError::ClosedByOtherProducer);
                    }
                    if transport_first_sequence != 0 {
                        return Err(DurableStreamProducerError::SequenceGap {
                            expected: 0,
                            actual: transport_first_sequence,
                        });
                    }
                    (stream.next_sequence, Some(index))
                };
            let delta = global_first_sequence
                .checked_sub(transport_first_sequence)
                .ok_or_else(|| {
                    DurableStreamProducerError::CorruptHistory(
                        "attached transport sequence exceeds global stream sequence".into(),
                    )
                })?;
            let nested = nested
                .into_iter()
                .map(|mut request| {
                    if let StreamRegistrationCoordinateV1::Nested {
                        parent_producer_sequence,
                        ..
                    } = &mut request.coordinate
                    {
                        *parent_producer_sequence = parent_producer_sequence
                            .checked_add(delta)
                            .ok_or(DurableStreamProducerError::CounterOverflow)?;
                    }
                    Ok(request)
                })
                .collect::<Result<Vec<_>, DurableStreamProducerError>>()?;
            owner
                .write_items_owned(
                    stream_id,
                    global_first_sequence,
                    payload,
                    nested
                        .into_iter()
                        .map(NestedStreamWriteV1::Register)
                        .collect(),
                    0,
                    Some(ExternalProducerV1 {
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

    pub(crate) async fn attached_global_sequence(
        &self,
        session_key: &StreamSessionKeyV1,
        stream_id: StreamId,
        transport_sequence: u64,
    ) -> Result<u64, DurableStreamProducerError> {
        let index = self
            .index_for_terminal(
                [
                    ProducerMetadataKey::Stream(stream_id),
                    ProducerMetadataKey::ExternalProducerSequence(
                        session_key.clone(),
                        stream_id,
                        ExternalProducerIdV1::Attached,
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
                ExternalProducerIdV1::Attached,
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
            .ok_or(DurableStreamProducerError::UnknownStream(stream_id))
    }

    async fn write_items_owned(
        &self,
        stream_id: StreamId,
        first_sequence: u64,
        payload: StreamItemsPayloadV1,
        nested_sources: Vec<NestedStreamWriteV1>,
        traversal_depth: usize,
        external_producer: Option<ExternalProducerV1>,
        index: Option<MutexGuard<'_, ProducerStreamIndex>>,
    ) -> Result<ProducerWriteOutcomeV1<Vec<StreamOffsetV1>>, DurableStreamProducerError> {
        validate_items_payload(&payload)?;
        let item_count = payload.logical_item_count() as u64;
        let nested = nested_sources
            .iter()
            .filter_map(|source| match source {
                NestedStreamWriteV1::Register(request) => Some(request.clone()),
                NestedStreamWriteV1::Forward(_) => None,
            })
            .collect::<Vec<_>>();

        let mut keys = vec![ProducerMetadataKey::Stream(stream_id)];
        for source in &nested_sources {
            match source {
                NestedStreamWriteV1::Register(request) => {
                    keys.extend(ProducerMetadataKey::registration(request))
                }
                NestedStreamWriteV1::Forward(handle) => {
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
            return Err(DurableStreamProducerError::RegistrationDivergence);
        }
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
                            NestedStreamWriteV1::Register(request) => index
                                .registrations
                                .get(stream_id)
                                .is_some_and(|record| registration_matches(record, request)),
                            NestedStreamWriteV1::Forward(handle) => stream_id == &handle.stream_id,
                        },
                    );
                drop(index);
                if !matches {
                    return Err(DurableStreamProducerError::EventConflict);
                }
                let events = self.materialize_item_batch(&record).await?;
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
                self.publish_repair(stream_id, vec![terminal]).await?;
                return Err(error);
            }
            return Err(DurableStreamProducerError::EventConflict);
        }
        index.ensure_producer_write_allowed()?;
        if session_finished {
            return Err(DurableStreamProducerError::SessionFinished(session_key));
        }
        if stream.terminal {
            let terminal = stream
                .terminal_event
                .as_ref()
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
        let external_session_key = session_key.clone();
        self.begin_durable_effect();
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
                let resulting_offset = *offsets.last().expect("validated input contains an item");
                let payload_for_high_water = payload_for_entry.clone();
                records.push(DurableStreamOplogRecord::Items(
                    entity_parent_start_index,
                    StreamItemsRecordV1 {
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
                    let resulting_offset = StreamOffsetV1::new(
                        item_index,
                        u32::try_from(logical_item_count - 1)
                            .expect("validated stream batch length fits in u32"),
                    );
                    records.push(DurableStreamOplogRecord::Session(
                        entity_parent_start_index,
                        Box::new(StreamSessionRecordV1::InputHighWater(
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
                        )),
                    ));
                }
                if let Some(producer) = external_producer {
                    records.push(DurableStreamOplogRecord::Session(
                        entity_parent_start_index,
                        Box::new(StreamSessionRecordV1::ExternalProducerState(
                            StreamExternalProducerStateRecordV1 {
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
            .map_err(DurableStreamProducerError::Oplog)?;
        self.commit().await;

        let (item_events, newly_registered_stream_count) = self
            .apply_committed_write_batch(&mut index, entries)
            .await?;
        let item_offsets = item_events.iter().map(|event| event.offset).collect();
        let publication = self.enqueue_events(stream_id, item_events, false)?;
        self.record_registered_streams(newly_registered_stream_count);
        drop(index);
        self.wait_for_publication(publication).await?;
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

    async fn apply_committed_write_batch(
        &self,
        index: &mut ProducerStreamIndex,
        entries: Vec<(OplogIndex, OplogEntry)>,
    ) -> Result<(Vec<CommittedProducerStreamEventV1>, usize), DurableStreamProducerError> {
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
                        .map_err(DurableStreamProducerError::Oplog)?;
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
                        .map_err(DurableStreamProducerError::Oplog)?;
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
                        .map_err(DurableStreamProducerError::Oplog)?;
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
                        .map_err(DurableStreamProducerError::Oplog)?;
                    index.apply_session_references(entity_parent_start_index, &record)?;
                    match record {
                        StreamSessionRecordV1::ExternalProducerState(record) => {
                            index.apply_external_producer_state(&record);
                        }
                        StreamSessionRecordV1::InputHighWater(_) => {}
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
        Ok((events, count))
    }

    pub(crate) async fn append_external_input(
        self: &Arc<Self>,
        session_key: &StreamSessionKeyV1,
        stream_id: StreamId,
        payload: Option<StreamItemsPayloadV1>,
        close: bool,
        producer: Option<ExternalProducerV1>,
    ) -> Result<ExternalAppendOutcomeV1, DurableStreamProducerError> {
        if let Some(payload) = &payload {
            match payload {
                StreamItemsPayloadV1::Values(values) => {
                    if values.is_empty() {
                        return Err(DurableStreamProducerError::InvalidValueBatch);
                    }
                    if values
                        .iter()
                        .any(|value| value.len() > MAX_DURABLE_STREAM_ITEM_SIZE)
                    {
                        return Err(DurableStreamProducerError::ItemTooLarge);
                    }
                }
                StreamItemsPayloadV1::PackedU8(_) => validate_items_payload(payload)?,
            }
        }
        let retained_bytes = match &payload {
            Some(StreamItemsPayloadV1::Values(values)) => {
                values.iter().map(Vec::len).sum::<usize>() * 2
            }
            Some(StreamItemsPayloadV1::PackedU8(bytes)) => {
                bytes.len() * (std::mem::size_of::<CommittedProducerStreamEventV1>() + 2)
            }
            None => 0,
        } + producer.as_ref().map_or(0, |producer| match &producer.id {
            ExternalProducerIdV1::Client(id) => id.len(),
            ExternalProducerIdV1::Attached => 0,
        });
        let session_key = session_key.clone();
        self.run_owned(retained_bytes, move |owner| async move {
            owner
                .append_external_input_owned(&session_key, stream_id, payload, close, producer)
                .await
        })
        .await
    }

    async fn append_external_input_owned(
        &self,
        session_key: &StreamSessionKeyV1,
        stream_id: StreamId,
        payload: Option<StreamItemsPayloadV1>,
        close: bool,
        producer: Option<ExternalProducerV1>,
    ) -> Result<ExternalAppendOutcomeV1, DurableStreamProducerError> {
        let mut keys = vec![ProducerMetadataKey::Stream(stream_id)];
        if let Some(producer) = &producer {
            if matches!(&producer.id, ExternalProducerIdV1::Client(id) if id.is_empty()) {
                return Err(DurableStreamProducerError::InvalidValueBatch);
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
            return Ok(ExternalAppendOutcomeV1::NotFound);
        };
        if index.stream_sessions.get(&stream_id) != Some(session_key) {
            return Ok(ExternalAppendOutcomeV1::NotFound);
        }
        if registration.source_kind != StreamSourceKindV1::ExternalInlineInput
            && !(registration.source_kind == StreamSourceKindV1::Nested
                && producer
                    .as_ref()
                    .is_some_and(|producer| producer.id == ExternalProducerIdV1::Attached))
        {
            return Err(DurableStreamProducerError::InvalidHandle);
        }
        let stream = &index.streams[&stream_id];
        index.ensure_producer_write_allowed()?;
        if payload.is_none() && !close {
            return Err(DurableStreamProducerError::InvalidValueBatch);
        }
        if stream.terminal {
            if close
                && let Some(event) = &stream.terminal_event
                && matches!(
                    event.payload,
                    CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::Ok)
                )
            {
                if payload.is_none() && producer.is_none() {
                    return Ok(ExternalAppendOutcomeV1::Duplicate {
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
                        return Ok(ExternalAppendOutcomeV1::Duplicate {
                            offset: event.offset,
                            highest_sequence: Some(head.last_sequence),
                        });
                    }
                }
            }
            return Ok(ExternalAppendOutcomeV1::Closed);
        }
        if index.finished_sessions.contains(session_key) {
            return Ok(ExternalAppendOutcomeV1::Closed);
        }
        if let Some(request) = &producer {
            let identity = (session_key.clone(), stream_id, request.id.clone());
            if let Some(head) = index.external_producer_heads.get(&identity) {
                if request.epoch < head.epoch {
                    return Ok(ExternalAppendOutcomeV1::EpochFenced(head.epoch));
                }
                if request.epoch == head.epoch && request.sequence <= head.last_sequence {
                    if request.id == ExternalProducerIdV1::Attached {
                        // Attached item retries validate their original payload through
                        // write_items_owned; an end frame cannot stand in for an item.
                        return Err(DurableStreamProducerError::EventConflict);
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
                            DurableStreamProducerError::CorruptHistory(
                                "external producer sequence has no original offset".into(),
                            )
                        })?;
                    return Ok(ExternalAppendOutcomeV1::Duplicate {
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
                    return Ok(ExternalAppendOutcomeV1::SeqGap {
                        expected,
                        received: request.sequence,
                    });
                }
            } else if request.sequence != 0 {
                return Ok(ExternalAppendOutcomeV1::SeqGap {
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
            .ok_or(DurableStreamProducerError::CounterOverflow)?;
        let producer_fingerprint = self.producer_fingerprint;
        let session = session_key.clone();
        if let Some(producer) = &producer {
            producer
                .sequence
                .checked_add(1)
                .ok_or(DurableStreamProducerError::CounterOverflow)?;
        }
        let producer_record = producer.clone();
        let entity_parent_start_index = index.entity_parent_start_index(stream_id)?;
        self.begin_durable_effect();
        let entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |first_index| {
                let mut records = Vec::new();
                let mut resulting_offset = None;
                if let Some(payload) = &payload {
                    match payload {
                        StreamItemsPayloadV1::Values(values) => {
                            for (position, value) in values.iter().enumerate() {
                                let item_index =
                                    OplogIndex::from_u64(first_index.as_u64() + position as u64);
                                let offset = StreamOffsetV1::new(item_index, 0);
                                resulting_offset = Some(offset);
                                records.push(DurableStreamOplogRecord::Items(
                                    entity_parent_start_index,
                                    StreamItemsRecordV1 {
                                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                                        stream_id,
                                        producer_fingerprint,
                                        first_sequence: first_sequence + position as u64,
                                        nested_stream_ids: Vec::new(),
                                        newly_registered_stream_ids: Vec::new(),
                                        payload: StreamItemsPayloadV1::Values(vec![value.clone()]),
                                        offsets: vec![offset],
                                    },
                                ));
                            }
                        }
                        StreamItemsPayloadV1::PackedU8(_) => {
                            let offsets = (0..payload.logical_item_count())
                                .map(|sub| StreamOffsetV1::new(first_index, sub as u32))
                                .collect::<Vec<_>>();
                            resulting_offset = offsets.last().copied();
                            records.push(DurableStreamOplogRecord::Items(
                                entity_parent_start_index,
                                StreamItemsRecordV1 {
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
                    resulting_offset = Some(StreamOffsetV1::new(end_index, 0));
                }
                let resulting_offset =
                    resulting_offset.expect("external append has payload or terminal");
                records.push(DurableStreamOplogRecord::Session(
                    entity_parent_start_index,
                    Box::new(StreamSessionRecordV1::InputHighWater(
                        StreamSessionInputHighWaterRecordV1 {
                            format_version: DURABLE_STREAM_FORMAT_VERSION,
                            session_key: session.clone(),
                            stream_id,
                            epoch: 1,
                            first_sequence,
                            payload: payload
                                .unwrap_or_else(|| StreamItemsPayloadV1::Values(Vec::new())),
                            high_water: InputStreamHighWaterV1 {
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
                        StreamEndRecordV1 {
                            format_version: DURABLE_STREAM_FORMAT_VERSION,
                            stream_id,
                            producer_fingerprint,
                            sequence: terminal_sequence,
                            offset: resulting_offset,
                            authored_by: StreamTerminalAuthorV1::Protocol,
                            result: StreamEndResultV1::Ok,
                        },
                    ));
                }
                if let Some(producer) = producer_record {
                    records.push(DurableStreamOplogRecord::Session(
                        entity_parent_start_index,
                        Box::new(StreamSessionRecordV1::ExternalProducerState(
                            StreamExternalProducerStateRecordV1 {
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
            .map_err(DurableStreamProducerError::Oplog)?;
        self.commit().await;

        let (events, _) = self
            .apply_committed_write_batch(&mut index, entries)
            .await?;
        let offset = events
            .last()
            .expect("external append committed no result offset")
            .offset;
        let publication = self.enqueue_events(stream_id, events, false)?;
        if close {
            self.record_terminal_streams(1);
        }
        drop(index);
        self.wait_for_publication(publication).await?;
        Ok(ExternalAppendOutcomeV1::Accepted(offset))
    }

    async fn commit_resource_exhausted_terminal(
        &self,
        mut index: MutexGuard<'_, ProducerStreamIndex>,
        stream_id: StreamId,
        sequence: u64,
    ) -> Result<(), DurableStreamProducerError> {
        let result = StreamEndResultV1::ErrorContext(resource_exhausted_error_context()?);
        let entity_parent_start_index = index.entity_parent_start_index(stream_id)?;
        let producer_fingerprint = self.producer_fingerprint;
        self.begin_durable_effect();
        let mut entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |oplog_index| {
                vec![DurableStreamOplogRecord::End(
                    entity_parent_start_index,
                    StreamEndRecordV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        stream_id,
                        producer_fingerprint,
                        sequence,
                        offset: StreamOffsetV1::new(oplog_index, 0),
                        authored_by: StreamTerminalAuthorV1::Protocol,
                        result,
                    },
                )]
            }))
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        self.commit().await;
        let (oplog_index, entry) = entries
            .pop()
            .expect("resource exhaustion terminal batch returned no oplog entry");
        let OplogEntry::StreamEnd {
            entity_parent_start_index,
            record,
            ..
        } = entry
        else {
            unreachable!("resource exhaustion terminal builder returned a different entry")
        };
        let record = self
            .oplog
            .download_payload(record)
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        let event = index.apply_end(
            oplog_index,
            entity_parent_start_index,
            record,
            self.producer_fingerprint,
        )?;
        let publication = self.enqueue_events(stream_id, vec![event], false)?;
        self.record_terminal_streams(1);
        drop(index);
        self.wait_for_publication(publication).await?;
        self.finish_durable_effect();
        Ok(())
    }

    pub(crate) async fn end(
        &self,
        stream_id: StreamId,
        sequence: u64,
        result: StreamEndResultV1,
    ) -> Result<ProducerWriteOutcomeV1<StreamOffsetV1>, DurableStreamProducerError> {
        let memory = match &result {
            StreamEndResultV1::ErrorContext(bytes) => bytes.len(),
            _ => 0,
        };
        self.run_owned(memory, move |owner| async move {
            owner
                .end_authored(stream_id, sequence, result, StreamTerminalAuthorV1::Guest)
                .await
        })
        .await
    }

    async fn end_authored(
        &self,
        stream_id: StreamId,
        sequence: u64,
        result: StreamEndResultV1,
        authored_by: StreamTerminalAuthorV1,
    ) -> Result<ProducerWriteOutcomeV1<StreamOffsetV1>, DurableStreamProducerError> {
        let index = self.index_for_terminal([], stream_id).await?;
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
                .terminal_event
                .as_ref()
                .expect("terminal stream has no terminal event")
                .clone();
            let error = fenced_by_terminal(stream);
            drop(index);
            self.publish_repair(stream_id, vec![terminal]).await?;
            return Err(error);
        }
        validate_new_terminal(&index, stream_id, sequence)?;
        let entity_parent_start_index = index.entity_parent_start_index(stream_id)?;
        let producer_fingerprint = self.producer_fingerprint;
        self.begin_durable_effect();
        let mut entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |oplog_index| {
                vec![DurableStreamOplogRecord::End(
                    entity_parent_start_index,
                    StreamEndRecordV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        stream_id,
                        producer_fingerprint,
                        sequence,
                        offset: StreamOffsetV1::new(oplog_index, 0),
                        authored_by,
                        result,
                    },
                )]
            }))
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        self.commit().await;
        let (oplog_index, entry) = entries
            .pop()
            .expect("stream end batch returned no oplog entry");
        let OplogEntry::StreamEnd {
            entity_parent_start_index,
            record,
            ..
        } = entry
        else {
            unreachable!("stream end builder returned a different entry")
        };
        let record = self
            .oplog
            .download_payload(record)
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        let event = index.apply_end(
            oplog_index,
            entity_parent_start_index,
            record,
            self.producer_fingerprint,
        )?;
        let offset = event.offset;
        let publication = self.enqueue_events(stream_id, vec![event], false)?;
        self.record_terminal_streams(1);
        drop(index);
        self.wait_for_publication(publication).await?;
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
    async fn commit_cancel_locked(
        &self,
        mut index: MutexGuard<'_, ProducerStreamIndex>,
        stream_id: StreamId,
        sequence: u64,
        role: StreamCancelRoleV1,
        reason: StreamCancelReasonV1,
        details: Option<String>,
    ) -> Result<PendingCommittedCancellation, DurableStreamProducerError> {
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
                let publication = self.enqueue_events(stream_id, vec![event.clone()], true)?;
                drop(index);
                self.cancel_source(stream_id);
                return Ok(PendingCommittedCancellation {
                    stream_id,
                    sequence,
                    role,
                    reason,
                    publication,
                    event,
                    outcome: Ok(ProducerWriteOutcomeV1 {
                        value: offset,
                        replayed: true,
                    }),
                });
            }
            TerminalReplayDecision::Fenced(event) => {
                let error = DurableStreamProducerError::FencedByTerminal(event.payload.clone());
                let publication = self.enqueue_events(stream_id, vec![event.clone()], true)?;
                drop(index);
                self.cancel_source(stream_id);
                return Ok(PendingCommittedCancellation {
                    stream_id,
                    sequence,
                    role,
                    reason,
                    publication,
                    event,
                    outcome: Err(error),
                });
            }
        }
        index.ensure_producer_write_allowed()?;
        validate_new_terminal(&index, stream_id, sequence)?;
        let entity_parent_start_index = index.entity_parent_start_index(stream_id)?;
        let producer_fingerprint = self.producer_fingerprint;
        self.begin_durable_effect();
        let mut entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |oplog_index| {
                vec![DurableStreamOplogRecord::Cancel(
                    entity_parent_start_index,
                    StreamCancelRecordV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        stream_id,
                        producer_fingerprint,
                        sequence,
                        offset: StreamOffsetV1::new(oplog_index, 0),
                        authored_by: StreamTerminalAuthorV1::Protocol,
                        role,
                        reason,
                        details,
                    },
                )]
            }))
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        self.commit().await;
        let (oplog_index, entry) = entries
            .pop()
            .expect("stream cancellation batch returned no oplog entry");
        let OplogEntry::StreamCancel {
            entity_parent_start_index,
            record,
            ..
        } = entry
        else {
            unreachable!("stream cancellation builder returned a different entry")
        };
        let record = self
            .oplog
            .download_payload(record)
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        let event = index.apply_cancel(
            oplog_index,
            entity_parent_start_index,
            record,
            self.producer_fingerprint,
        )?;
        let offset = event.offset;
        let publication = self.enqueue_events(stream_id, vec![event.clone()], false)?;
        self.record_terminal_streams(1);
        drop(index);
        self.cancel_source(stream_id);
        Ok(PendingCommittedCancellation {
            stream_id,
            sequence,
            role,
            reason,
            publication,
            event,
            outcome: Ok(ProducerWriteOutcomeV1 {
                value: offset,
                replayed: false,
            }),
        })
    }

    pub(crate) async fn publish_committed_cancellation(
        &self,
        pending: PendingCommittedCancellation,
    ) -> Result<ProducerWriteOutcomeV1<StreamOffsetV1>, DurableStreamProducerError> {
        let offset = pending.event.offset;
        self.wait_for_publication(pending.publication).await?;
        if let Ok(outcome) = &pending.outcome {
            crate::metrics::durable_stream::record_producer_operation("cancel", outcome.replayed);
            tracing::debug!(
                stream_id = %pending.stream_id,
                sequence = pending.sequence,
                durable_offset = %offset,
                role = ?pending.role,
                reason = ?pending.reason,
                replayed = outcome.replayed,
                "Durable stream cancellation committed"
            );
        }
        pending.outcome
    }

    pub(crate) async fn commit_cancel_open(
        &self,
        stream_id: StreamId,
        role: StreamCancelRoleV1,
        reason: StreamCancelReasonV1,
        details: Option<String>,
    ) -> Result<Option<PendingCommittedCancellation>, DurableStreamProducerError> {
        self.run_lifecycle(
            details.as_ref().map_or(0, String::len),
            move |owner| async move {
                owner
                    .commit_cancel_open_owned(stream_id, role, reason, details)
                    .await
            },
        )
        .await
    }

    async fn commit_cancel_open_owned(
        &self,
        stream_id: StreamId,
        role: StreamCancelRoleV1,
        reason: StreamCancelReasonV1,
        details: Option<String>,
    ) -> Result<Option<PendingCommittedCancellation>, DurableStreamProducerError> {
        let index = self
            .index_for([ProducerMetadataKey::Stream(stream_id)])
            .await?;
        let stream = index
            .streams
            .get(&stream_id)
            .ok_or(DurableStreamProducerError::UnknownStream(stream_id))?;
        if stream.terminal {
            drop(index);
            self.cancel_source(stream_id);
            return Ok(None);
        }
        let sequence = stream.next_sequence;
        self.commit_cancel_locked(index, stream_id, sequence, role, reason, details)
            .await
            .map(Some)
    }

    pub(crate) async fn cancel_open(
        self: &Arc<Self>,
        stream_id: StreamId,
        role: StreamCancelRoleV1,
        reason: StreamCancelReasonV1,
        details: Option<String>,
    ) -> Result<(), DurableStreamProducerError> {
        self.run_lifecycle(
            details.as_ref().map_or(0, String::len),
            move |owner| async move {
                owner
                    .cancel_open_owned(stream_id, role, reason, details)
                    .await
            },
        )
        .await
    }

    async fn cancel_open_owned(
        &self,
        stream_id: StreamId,
        role: StreamCancelRoleV1,
        reason: StreamCancelReasonV1,
        details: Option<String>,
    ) -> Result<(), DurableStreamProducerError> {
        if let Some(pending) = self
            .commit_cancel_open(stream_id, role, reason, details)
            .await?
        {
            self.publish_committed_cancellation(pending).await?;
        }
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
        let memory = match &result {
            StreamEndResultV1::ErrorContext(bytes) => bytes.len(),
            _ => 0,
        };
        self.run_lifecycle(memory, move |owner| async move {
            owner.end_open_owned(stream_id, result).await
        })
        .await
    }

    async fn end_open_owned(
        &self,
        stream_id: StreamId,
        result: StreamEndResultV1,
    ) -> Result<(), DurableStreamProducerError> {
        let index = self
            .index_for([ProducerMetadataKey::Stream(stream_id)])
            .await?;
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
    ) -> Result<bool, DurableStreamProducerError> {
        let index = self.index_for_session_streams(session_key).await?;
        let Some(mappings) = index.session_stream_mappings.get(session_key) else {
            return Ok(false);
        };
        let candidates = mappings
            .iter()
            .filter_map(|(handle, role)| {
                (*role == SessionStreamRoleV1::Input
                    && mappings.contains(&(handle.clone(), SessionStreamRoleV1::Output))
                    && self.owns_handle_identity(handle))
                .then_some(handle.stream_id)
            })
            .collect::<Vec<_>>();
        Ok(index.session_stream_mappings[session_key]
            .iter()
            .any(|(handle, role)| {
                *role == SessionStreamRoleV1::Input
                    && candidates.contains(&handle.stream_id)
                    && index
                        .streams
                        .get(&handle.stream_id)
                        .is_some_and(|stream| !stream.terminal)
            }))
    }

    pub(crate) async fn finish_session(
        &self,
        session_key: StreamSessionKeyV1,
        entity_parent_start_index: Option<OplogIndex>,
        result: Result<(), Vec<u8>>,
        input_cancel_reason: StreamCancelReasonV1,
    ) -> Result<(), DurableStreamProducerError> {
        if self
            .index_for([ProducerMetadataKey::Session(session_key.clone())])
            .await?
            .finished_sessions
            .contains(&session_key)
        {
            return Ok(());
        }
        // The lazy batch retains one error, one record, and its encoding at a time,
        // independently of the number of output streams.
        let terminal_bytes = result
            .as_ref()
            .err()
            .map_or(0, |bytes| bytes.len())
            .max(b"output stream ended without a terminal".len());
        let memory = terminal_bytes.saturating_mul(4);
        self.run_lifecycle(memory, move |owner| async move {
            owner
                .finish_session_owned(
                    session_key,
                    entity_parent_start_index,
                    result,
                    input_cancel_reason,
                )
                .await
        })
        .await
    }

    async fn finish_session_owned(
        &self,
        session_key: StreamSessionKeyV1,
        entity_parent_start_index: Option<OplogIndex>,
        result: Result<(), Vec<u8>>,
        input_cancel_reason: StreamCancelReasonV1,
    ) -> Result<(), DurableStreamProducerError> {
        let mut index = self.index_for_session_streams(&session_key).await?;
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
                        *index
                            .entity_parent_start_indices
                            .get(stream_id)
                            .expect("session stream index points at missing attribution"),
                    )
                })
            })
            .collect::<Vec<_>>();
        open_streams.sort_by_key(|(stream_id, _, _, _)| *stream_id);
        if open_streams
            .iter()
            .any(|(_, _, _, attribution)| *attribution != entity_parent_start_index)
        {
            return Err(DurableStreamProducerError::CorruptHistory(
                "session stream attribution differs from its session".to_string(),
            ));
        }

        let producer_fingerprint = self.producer_fingerprint;
        let result_for_batch = Arc::new(result);
        let session_key_for_batch = session_key.clone();
        self.begin_durable_effect();
        let entries = self
            .oplog
            .add_durable_stream_batch_iter(Box::new(move |first_index| {
                let finished_result = result_for_batch.clone();
                let terminals = open_streams.into_iter().enumerate().map(
                    move |(position, (stream_id, role, sequence, stream_attribution))| {
                        let oplog_index =
                            OplogIndex::from_u64(first_index.as_u64() + position as u64);
                        match role {
                            SessionStreamRoleV1::Input => DurableStreamOplogRecord::Cancel(
                                stream_attribution,
                                StreamCancelRecordV1 {
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
                                },
                            ),
                            SessionStreamRoleV1::Output => {
                                let details = match result_for_batch.as_ref() {
                                    Ok(()) => b"output stream ended without a terminal".to_vec(),
                                    Err(details) => details.clone(),
                                };
                                DurableStreamOplogRecord::End(
                                    stream_attribution,
                                    StreamEndRecordV1 {
                                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                                        stream_id,
                                        producer_fingerprint,
                                        sequence,
                                        offset: StreamOffsetV1::new(oplog_index, 0),
                                        authored_by: StreamTerminalAuthorV1::Protocol,
                                        result: StreamEndResultV1::ErrorContext(details),
                                    },
                                )
                            }
                        }
                    },
                );
                let finished = std::iter::once_with(move || {
                    DurableStreamOplogRecord::Session(
                        entity_parent_start_index,
                        Box::new(StreamSessionRecordV1::Finished(
                            StreamSessionFinishedRecordV1 {
                                format_version: DURABLE_STREAM_FORMAT_VERSION,
                                session_key: session_key_for_batch,
                                result: Arc::unwrap_or_clone(finished_result),
                            },
                        )),
                    )
                });
                Box::new(terminals.chain(finished))
            }))
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        self.commit().await;

        let mut terminal_events = Vec::new();
        for (oplog_index, entry) in entries {
            match entry {
                OplogEntry::StreamEnd {
                    entity_parent_start_index,
                    record,
                    ..
                } => {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(DurableStreamProducerError::Oplog)?;
                    let event = index.apply_end(
                        oplog_index,
                        entity_parent_start_index,
                        record,
                        self.producer_fingerprint,
                    )?;
                    terminal_events.push(self.enqueue_events(
                        event.stream_id,
                        vec![event],
                        false,
                    )?);
                }
                OplogEntry::StreamCancel {
                    entity_parent_start_index,
                    record,
                    ..
                } => {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(DurableStreamProducerError::Oplog)?;
                    let event = index.apply_cancel(
                        oplog_index,
                        entity_parent_start_index,
                        record,
                        self.producer_fingerprint,
                    )?;
                    terminal_events.push(self.enqueue_events(
                        event.stream_id,
                        vec![event],
                        false,
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
                        .map_err(DurableStreamProducerError::Oplog)?;
                    index.apply_session_references(entity_parent_start_index, &record)?;
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
        let terminal_count = terminal_events.len();
        self.record_terminal_streams(terminal_count);
        drop(index);
        for publication in terminal_events {
            self.wait_for_publication(publication).await?;
        }
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
        self: &Arc<Self>,
        handle: DurableStreamHandleV1,
        after: Option<StreamOffsetV1>,
    ) -> Result<DurableCatchUpReader, DurableStreamProducerError> {
        self.validate_handle(&handle).await?;
        let bus = self.stream_bus(handle.stream_id).await?;
        self.validate_cursor(handle.stream_id, after).await?;
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
            history_source: join_high_water.map(|_| (self.clone(), handle)),
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
        let index = self
            .index_for([ProducerMetadataKey::Stream(handle.stream_id)])
            .await?;
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
        let index = self
            .index_for([
                ProducerMetadataKey::Stream(stream_id),
                ProducerMetadataKey::Position(stream_id, after.producer_oplog_index()),
            ])
            .await?;
        let high_water = index
            .streams
            .get(&stream_id)
            .and_then(|stream| stream.last_offset);
        if high_water.is_none_or(|high_water| after > high_water)
            || !after.producer_oplog_index().is_defined()
        {
            return Err(DurableStreamProducerError::CursorUnavailable);
        }
        if index.streams[&stream_id].terminal && high_water == Some(after) {
            return Ok(());
        }
        let valid = index
            .batch_positions
            .get(&(stream_id, after.producer_oplog_index()))
            .is_some_and(|(_, count)| u64::from(after.sub_index()) < *count);
        if valid {
            Ok(())
        } else {
            Err(DurableStreamProducerError::CursorUnavailable)
        }
    }

    fn enqueue_events(
        &self,
        stream_id: StreamId,
        events: Vec<CommittedProducerStreamEventV1>,
        replayed: bool,
    ) -> Result<PublicationReceipt, DurableStreamProducerError> {
        let bus = self.bus(stream_id)?;
        if !replayed {
            self.retain_committed_events(&events);
            bus.notify_committed();
        }
        if events.len() == 1 && events[0].is_terminal() {
            let event = events.into_iter().next().unwrap();
            let receipt =
                bus.defer_terminal(event.offset, replayed, self.terminal_progress.clone())?;
            bus.try_publish_deferred_terminal(self.queued_terminal(stream_id, event.offset))?;
            self.start_terminal_dispatcher();
            return Ok(receipt);
        }
        Ok(bus.enqueue_batch(
            events
                .into_iter()
                .map(|event| DurableLiveStreamEvent {
                    offset: event.offset,
                    payload: event,
                })
                .collect(),
            replayed,
            MUTATION_SCOPE
                .try_with(Arc::clone)
                .ok()
                .filter(|scope| std::ptr::eq(scope.producer.as_ref(), self)),
        ))
    }

    fn queued_terminal(
        &self,
        stream_id: StreamId,
        offset: StreamOffsetV1,
    ) -> QueuedDurableEvent<CommittedProducerStreamEventV1> {
        let oplog = self.oplog.clone();
        QueuedDurableEvent::Terminal {
            offset,
            load: Arc::new(move |offset| {
                let oplog = oplog.clone();
                Box::pin(async move {
                    metadata::read_terminal_event(oplog.as_ref(), stream_id, offset)
                        .await
                        .map_err(|_| DurableLiveStreamBusError::PublicationAborted)
                })
            }),
        }
    }

    fn start_terminal_dispatcher(&self) {
        if self
            .terminal_dispatcher_started
            .swap(true, Ordering::AcqRel)
        {
            return;
        }
        let producer = self.self_weak.clone();
        let progress = self.terminal_progress.clone();
        tokio::spawn(async move {
            loop {
                let changed = progress.notified();
                tokio::pin!(changed);
                changed.as_mut().enable();
                let mut cursor = None;
                let mut pending = false;
                loop {
                    let Some(producer) = producer.upgrade() else {
                        return;
                    };
                    let page: Vec<_> = producer
                        .buses
                        .read()
                        .expect("durable stream bus map lock poisoned")
                        .range((
                            cursor.map_or(std::ops::Bound::Unbounded, std::ops::Bound::Excluded),
                            std::ops::Bound::Unbounded,
                        ))
                        .take(32)
                        .map(|(id, bus)| (*id, bus.clone()))
                        .collect();
                    let last_page = page.len() < 32;
                    for (stream_id, bus) in page {
                        cursor = Some(stream_id);
                        if !bus.has_pending_deferred_terminal() {
                            continue;
                        }
                        let result = async {
                            producer.ensure_healthy()?;
                            if let Some(offset) = bus.ready_deferred_terminal()? {
                                bus.try_publish_deferred_terminal(
                                    producer.queued_terminal(stream_id, offset),
                                )?;
                            }
                            Ok::<(), DurableStreamProducerError>(())
                        }
                        .await;
                        if result.is_err() {
                            producer.poison();
                            bus.fail_deferred_terminal(
                                DurableLiveStreamBusError::PublicationAborted,
                            );
                        }
                        pending |= bus.has_pending_deferred_terminal();
                    }
                    if last_page {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
                if pending {
                    // A transient bus-state lock need not generate a reader notification.
                    tokio::select! {
                        _ = &mut changed => {},
                        _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {},
                    }
                } else {
                    changed.await;
                }
            }
        });
    }

    async fn wait_for_publication(
        &self,
        publication: PublicationReceipt,
    ) -> Result<(), DurableStreamProducerError> {
        let scope = MUTATION_SCOPE
            .try_with(Arc::clone)
            .ok()
            .filter(|scope| std::ptr::eq(scope.producer.as_ref(), self));
        if let Some(scope) = scope {
            scope
                .publications
                .lock()
                .expect("publication receipt list lock poisoned")
                .push(publication);
        } else {
            publication
                .await
                .map_err(|_| DurableLiveStreamBusError::PublicationAborted)??;
        }
        Ok(())
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

    async fn stream_bus(
        &self,
        stream: StreamId,
    ) -> Result<Arc<DurableLiveStreamBus<CommittedProducerStreamEventV1>>, DurableStreamProducerError>
    {
        let _index = self
            .index_for([ProducerMetadataKey::Stream(stream)])
            .await?;
        self.bus(stream)
    }

    async fn publish_repair(
        &self,
        stream_id: StreamId,
        events: Vec<CommittedProducerStreamEventV1>,
    ) -> Result<(), DurableStreamProducerError> {
        let index = self
            .index_for([ProducerMetadataKey::Stream(stream_id)])
            .await?;
        let publication = self.enqueue_events(stream_id, events, true)?;
        drop(index);
        self.wait_for_publication(publication).await?;
        Ok(())
    }
}

impl Drop for DurableStreamProducer {
    fn drop(&mut self) {
        self.terminal_progress.notify_one();
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
    if let Some(event) = &stream.terminal_event
        && event.producer_sequence == sequence
    {
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
    if sequence < stream.next_sequence {
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
            .terminal_event
            .as_ref()
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
    async fn journal_lag_events(
        &self,
        handle: &DurableStreamHandleV1,
        after: Option<StreamOffsetV1>,
    ) -> Result<usize, DurableStreamProducerError>;

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

    #[cfg(test)]
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
    metadata: Arc<DurableStreamProducer>,
}

impl RoutedAttachedStreamSegmentSource {
    pub(crate) fn new(
        rpc: Arc<dyn Rpc>,
        mapping: StreamSessionMappingRecordV1,
        auth_ctx: AuthCtx,
        metadata: Arc<DurableStreamProducer>,
    ) -> Self {
        Self {
            rpc,
            mapping,
            auth_ctx,
            metadata,
        }
    }

    async fn read_routed(
        &self,
        request: AttachedStreamSegmentRequestV1,
    ) -> Result<Vec<u8>, DurableStreamProducerError> {
        let mut delay = std::time::Duration::from_millis(100);
        loop {
            match self.rpc.read_durable_stream_segment(
                golem_common::model::durable_stream::DurableStreamReadRequestV1::AttachedConsumer(Box::new(request.clone())),
                &self.auth_ctx,
            ).await {
                Ok(payload) => return Ok(payload),
                Err(DurableStreamReadError::Other(error)) => return Err(DurableStreamProducerError::Oplog(error.to_string())),
                Err(DurableStreamReadError::Unavailable) => {
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(std::time::Duration::from_secs(1));
                }
            }
        }
    }
}

#[async_trait]
impl AttachedStreamSegmentSource for RoutedAttachedStreamSegmentSource {
    async fn journal_lag_events(
        &self,
        handle: &DurableStreamHandleV1,
        after: Option<StreamOffsetV1>,
    ) -> Result<usize, DurableStreamProducerError> {
        if self.mapping.handle != *handle {
            return Err(DurableStreamProducerError::InvalidHandle);
        }
        self.metadata.journal_lag_events(handle, after).await
    }

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
            .read_routed(AttachedStreamSegmentRequestV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                attachment: attachment.clone(),
                mapping: self.mapping.clone(),
                after,
                through,
                wait_for_events: false,
            })
            .await?;
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
            .read_routed(AttachedStreamSegmentRequestV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                attachment: attachment.clone(),
                mapping: self.mapping.clone(),
                after,
                through: None,
                wait_for_events: true,
            })
            .await?;
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

    #[cfg(test)]
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

    async fn journal_summary(
        &self,
        _key: &StreamAttachmentKeyV1,
    ) -> Result<Option<ConsumerJournalSummary>, DurableStreamProducerError> {
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

    pub(crate) async fn committed_cancellation_status(
        &self,
        key: &StreamAttachmentKeyV1,
        mapping: &StreamSessionMappingRecordV1,
        intent: &golem_common::model::durable_stream::StreamConsumerCancelIntentRecordV1,
    ) -> Result<ConsumerAttachmentStatus, DurableStreamProducerError> {
        self.inspect_status(key, Some(mapping), Some(intent)).await
    }

    #[tracing::instrument(name = "durable_stream.consumer.status", level = "debug", skip_all)]
    async fn inspect_status(
        &self,
        key: &StreamAttachmentKeyV1,
        expected_mapping: Option<&StreamSessionMappingRecordV1>,
        expected_cancel: Option<
            &golem_common::model::durable_stream::StreamConsumerCancelIntentRecordV1,
        >,
    ) -> Result<ConsumerAttachmentStatus, DurableStreamProducerError> {
        let session_owner = OwnedAgentId::new(
            key.session_key.callee_environment_id,
            &key.session_key.callee,
        );
        let Some(session_mode) = self
            .worker_service
            .get_agent_mode(&session_owner)
            .await
            .map_err(|err| DurableStreamProducerError::Oplog(err.to_string()))?
        else {
            return Ok(ConsumerAttachmentStatus::Missing);
        };
        if session_mode != AgentMode::Durable {
            return Ok(ConsumerAttachmentStatus::IncarnationMismatch);
        }
        let session_index = self.oplog_service.stream_session_index().ok_or_else(|| {
            DurableStreamProducerError::Oplog(
                "stream session index service is unavailable".to_string(),
            )
        })?;
        let identity = session_index
            .lookup_producer_identity(&session_owner, session_mode)
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        if identity.producer_fingerprint != key.session_key.callee_fingerprint {
            return Ok(ConsumerAttachmentStatus::IncarnationMismatch);
        }
        let Some(status) = session_index
            .lookup_latest(
                &session_owner,
                session_mode,
                &key.session_key.idempotency_key,
            )
            .await
            .map_err(DurableStreamProducerError::Oplog)?
        else {
            return Ok(ConsumerAttachmentStatus::Missing);
        };
        if status.session_key.as_ref() != Some(&key.session_key) {
            return Ok(ConsumerAttachmentStatus::Missing);
        }
        if let Some(error) = status.lifecycle_error {
            return Err(DurableStreamProducerError::CorruptHistory(error));
        }
        let Some(prepared_attempt_id) = status.prepared_attempt_id else {
            return Ok(ConsumerAttachmentStatus::Missing);
        };
        let primary_attachment_id = AttachmentId::primary(
            key.session_key.callee_environment_id,
            &key.session_key.callee,
            &key.session_key.idempotency_key,
        )
        .map_err(|error| DurableStreamProducerError::CorruptHistory(error.to_string()))?;
        if key.attachment_id != primary_attachment_id {
            return Ok(ConsumerAttachmentStatus::IncarnationMismatch);
        }
        match (
            status.initial_attachment_epoch,
            status.initial_attachment_attempt_id,
            status.initial_pending_invocation_oplog_index,
        ) {
            (None, None, None) => {}
            (Some(_), Some(initial_attempt_id), Some(pending_index)) => {
                if initial_attempt_id != prepared_attempt_id
                    || status.validated_initial_pending_invocation != Some(pending_index)
                {
                    return Err(DurableStreamProducerError::CorruptHistory(
                        "durable Attached record does not identify its Prepared attempt and pending invocation"
                            .to_string(),
                    ));
                }
            }
            _ => {
                return Err(DurableStreamProducerError::CorruptHistory(
                    "durable attachment lifecycle index is incomplete".to_string(),
                ));
            }
        }
        let attachment_authority = status
            .attachment_epoch
            .zip(status.attachment_attempt_id)
            .zip(status.attachment_attached)
            .map(|((epoch, attempt_id), attached)| (epoch, attempt_id, attached));
        if expected_cancel.is_none()
            && attachment_authority.is_some_and(|(epoch, _, _)| epoch != key.epoch)
        {
            return Ok(ConsumerAttachmentStatus::EpochMismatch);
        }

        let consumer = OwnedAgentId::new(key.consumer_environment_id, &key.consumer);
        let Some(agent_mode) = self
            .worker_service
            .get_agent_mode(&consumer)
            .await
            .map_err(|err| DurableStreamProducerError::Oplog(err.to_string()))?
        else {
            return Ok(ConsumerAttachmentStatus::Missing);
        };
        if key.consumer_invocation.callee_environment_id != key.consumer_environment_id
            || key.consumer_invocation.callee != key.consumer
            || key.consumer_invocation.callee_fingerprint != key.expected_consumer_fingerprint
        {
            return Ok(ConsumerAttachmentStatus::IncarnationMismatch);
        }
        if agent_mode != AgentMode::Durable {
            return Ok(ConsumerAttachmentStatus::IncarnationMismatch);
        }
        let identity = session_index
            .lookup_producer_identity(&consumer, agent_mode)
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        if identity.producer_fingerprint != key.expected_consumer_fingerprint {
            return Ok(ConsumerAttachmentStatus::IncarnationMismatch);
        }
        let metadata = self
            .worker_service
            .lookup_durable_stream_control_metadata(&consumer, agent_mode, &key.session_key)
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        if !metadata.covered_through.is_defined() {
            return Ok(ConsumerAttachmentStatus::Missing);
        }
        if let Some(intent) = expected_cancel {
            let mapping = expected_mapping.expect("cancellation inspection requires a mapping");
            return metadata
                .has_committed_cancellation(key, mapping, intent)
                .map(|matches| {
                    if matches {
                        ConsumerAttachmentStatus::Active
                    } else {
                        ConsumerAttachmentStatus::Missing
                    }
                })
                .map_err(DurableStreamProducerError::CorruptHistory);
        }
        let topology = metadata
            .topology_status(key, expected_mapping)
            .map_err(DurableStreamProducerError::CorruptHistory)?;
        if matches!(
            topology,
            ConsumerAttachmentStatus::EpochMismatch | ConsumerAttachmentStatus::IncarnationMismatch
        ) {
            return Ok(topology);
        }
        if metadata.consumer_deleting.as_ref().is_some_and(|record| {
            record.consumer_environment_id == key.consumer_environment_id
                && record.consumer == key.consumer
                && record.consumer_fingerprint == key.expected_consumer_fingerprint
        }) {
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
        self.inspect_status(key, expected_mapping, None).await
    }

    #[tracing::instrument(
        name = "durable_stream.consumer.journal_inspection",
        level = "debug",
        skip_all
    )]
    async fn journal_inspection(
        &self,
        key: &StreamAttachmentKeyV1,
    ) -> Result<Option<ConsumerJournalInspection>, DurableStreamProducerError> {
        let consumer = OwnedAgentId::new(key.consumer_environment_id, &key.consumer);
        let Some(metadata) = self
            .worker_service
            .get(&consumer)
            .await
            .map_err(|err| DurableStreamProducerError::Oplog(err.to_string()))?
        else {
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

    async fn journal_summary(
        &self,
        key: &StreamAttachmentKeyV1,
    ) -> Result<Option<ConsumerJournalSummary>, DurableStreamProducerError> {
        let consumer = OwnedAgentId::new(key.consumer_environment_id, &key.consumer);
        let Some(metadata) = self
            .worker_service
            .get(&consumer)
            .await
            .map_err(|err| DurableStreamProducerError::Oplog(err.to_string()))?
        else {
            return Ok(None);
        };
        if metadata.initial_worker_metadata.fingerprint != key.expected_consumer_fingerprint
            || metadata.initial_worker_metadata.agent_mode != AgentMode::Durable
        {
            return Ok(None);
        }
        let (_, mut rows) = self
            .worker_service
            .lookup_durable_stream_producer_metadata(
                &consumer,
                AgentMode::Durable,
                vec![ProducerMetadataKey::ConsumerHead(
                    key.session_key.clone(),
                    key.stream_id,
                )],
            )
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        let Some(ProducerMetadataRow::ConsumerHead(head)) = rows.pop().flatten() else {
            return Ok(None);
        };
        Ok(Some(ConsumerJournalSummary {
            event_count: head.next_read_ordinal,
            last_offset: head.last_source_offset,
            terminal: head.terminal,
            source_unavailable: head.source_unavailable.is_some(),
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

    #[cfg(test)]
    async fn inspect_attachments(&self) -> Vec<StreamAttachmentViewV1> {
        self.index.lock().await.attachment_views()
    }
}

impl DurableStreamProducer {
    pub(crate) async fn has_active_attachment(
        &self,
        session: &StreamSessionKeyV1,
        handle: &DurableStreamHandleV1,
    ) -> Result<bool, DurableStreamProducerError> {
        let index = self
            .index_for([
                ProducerMetadataKey::Stream(handle.stream_id),
                ProducerMetadataKey::ActiveAttachmentCount(session.clone(), handle.stream_id),
            ])
            .await?;
        if !index
            .registrations
            .get(&handle.stream_id)
            .is_some_and(|registration| {
                registration.handle.producer_environment_id == handle.producer_environment_id
                    && registration.handle.producer == handle.producer
                    && registration.handle.expected_producer_fingerprint
                        == handle.expected_producer_fingerprint
            })
        {
            return Err(DurableStreamProducerError::InvalidHandle);
        }
        Ok(index
            .active_attachments_by_session_stream
            .get(&(session.clone(), handle.stream_id))
            .copied()
            .unwrap_or_default()
            > 0)
    }

    pub(crate) async fn deletion_started(&self) -> bool {
        let index = self.index.lock().await;
        index.deleting || index.consumer_deleting
    }

    pub(crate) async fn deletion_diagnostics(
        &self,
    ) -> Result<StreamDeletionDiagnosticsV1, DurableStreamProducerError> {
        let index = self.index_for_cleanup().await?;
        let mut cascade_completed = index
            .cascade_outbox
            .iter()
            .map(|(key, result)| (key.clone(), result.clone()))
            .collect::<Vec<_>>();
        cascade_completed.sort_by(|(left, _), (right, _)| {
            attachment_sort_key(left).cmp(&attachment_sort_key(right))
        });
        Ok(StreamDeletionDiagnosticsV1 {
            deleting: index.deleting,
            attachments: index.attachment_views(),
            cascade_completed,
        })
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
                            .offsets()
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
        self.run_lifecycle(0, move |owner| async move {
            owner
                .commit_deletion_barrier_owned(now_millis, require_no_dependents)
                .await
        })
        .await
    }

    async fn commit_deletion_barrier_owned(
        &self,
        now_millis: u64,
        require_no_dependents: bool,
    ) -> Result<(), DurableStreamProducerError> {
        let mut index = self.index_for_cleanup().await?;
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
                (!stream.terminal).then_some((
                    *stream_id,
                    stream.next_sequence,
                    *index
                        .entity_parent_start_indices
                        .get(stream_id)
                        .expect("stream index points at missing attribution"),
                ))
            })
            .collect::<Vec<_>>();
        open_streams.sort_by_key(|(stream_id, _, _)| *stream_id);
        let environment_id = self.environment_id;
        let producer = self.producer.clone();
        let producer_fingerprint = self.producer_fingerprint;
        self.begin_durable_effect();
        let entries = self
            .oplog
            .add_durable_stream_batch(Box::new(move |first_index| {
                let mut records = Vec::with_capacity(open_streams.len() + 1);
                for (position, (stream_id, sequence, entity_parent_start_index)) in
                    open_streams.into_iter().enumerate()
                {
                    let oplog_index = OplogIndex::from_u64(first_index.as_u64() + position as u64);
                    records.push(DurableStreamOplogRecord::Cancel(
                        entity_parent_start_index,
                        StreamCancelRecordV1 {
                            format_version: DURABLE_STREAM_FORMAT_VERSION,
                            stream_id,
                            producer_fingerprint,
                            sequence,
                            offset: StreamOffsetV1::new(oplog_index, 0),
                            authored_by: StreamTerminalAuthorV1::Protocol,
                            role: StreamCancelRoleV1::System,
                            reason: StreamCancelReasonV1::ProducerDeleting,
                            details: None,
                        },
                    ));
                }
                records.push(DurableStreamOplogRecord::Session(
                    None,
                    Box::new(StreamSessionRecordV1::ProducerDeleting(
                        StreamProducerDeletingRecordV1 {
                            format_version: DURABLE_STREAM_FORMAT_VERSION,
                            producer_environment_id: environment_id,
                            producer,
                            producer_fingerprint,
                            deleting_at_millis: now_millis,
                        },
                    )),
                ));
                records
            }))
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        self.commit().await;
        let mut terminal_events = Vec::new();
        for (oplog_index, entry) in entries {
            match entry {
                OplogEntry::StreamCancel {
                    entity_parent_start_index,
                    record,
                    ..
                } => {
                    let record = self
                        .oplog
                        .download_payload(record)
                        .await
                        .map_err(DurableStreamProducerError::Oplog)?;
                    terminal_events.push(index.apply_cancel(
                        oplog_index,
                        entity_parent_start_index,
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
        index.complete_for_deletion = true;
        let terminal_count = terminal_events.len();
        for event in terminal_events {
            self.cancel_source(event.stream_id);
            // The terminal dispatcher owns delivery. Deletion waits for the committed
            // barrier and cascade records, never for a live reader to make progress.
            drop(self.enqueue_events(event.stream_id, vec![event], false)?);
        }
        self.record_terminal_streams(terminal_count);
        drop(index);
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
        self.run_lifecycle(0, move |owner| async move {
            owner
                .commit_cascade_outbox_owned(key, now_millis, result)
                .await
        })
        .await
    }

    async fn commit_cascade_outbox_owned(
        &self,
        key: StreamAttachmentKeyV1,
        now_millis: u64,
        result: StreamCascadeDependentResultV1,
    ) -> Result<(), DurableStreamProducerError> {
        let mut index = self.index.lock().await;
        self.ensure_healthy()?;
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
        let entity_parent_start_index = index.entity_parent_start_index(stream_id)?;
        let record = StreamSessionRecordV1::CascadeOutbox(StreamCascadeOutboxRecordV1 {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            key,
            completed_at_millis: now_millis,
            result,
        });
        self.begin_durable_effect();
        self.oplog
            .add(OplogEntry::stream_session(
                entity_parent_start_index,
                OplogPayload::Inline(Box::new(record.clone())),
            ))
            .await;
        self.commit().await;
        index.apply_session_references(entity_parent_start_index, &record)?;
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
        self.index_for([ProducerMetadataKey::Attachment(
            key.attachment_id,
            key.stream_id,
            key.consumer_environment_id,
            key.consumer.clone(),
        )])
        .await?
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
        self.ensure_healthy()?;
        let (deleting, candidates) =
            if let Some(candidates) = self.indexed_attachment_candidates(batch_size).await? {
                candidates
            } else {
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
                            ProducerJournalSummary {
                                event_count: stream.next_sequence + u64::from(stream.terminal),
                                last_offset: stream.last_offset,
                                terminal: stream.terminal,
                            },
                        )
                    })
                    .collect::<Vec<_>>();
                candidates.sort_by_key(|(attachment, _)| {
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
        for (attachment, producer_summary) in candidates {
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
            let journal_complete = if producer_summary.terminal {
                match probe.journal_summary(&attachment.key).await {
                    Ok(Some(consumer)) => {
                        // Receive journals contain ordered, non-repeating producer offsets.
                        // Equal logical counts and the same terminal therefore imply completion.
                        !consumer.source_unavailable
                            && consumer.terminal
                            && consumer.event_count == producer_summary.event_count
                            && consumer.last_offset == producer_summary.last_offset
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
    #[tracing::instrument(name = "durable_stream.read_segment", level = "debug", skip_all)]
    async fn read_segment(
        &self,
        handle: &DurableStreamHandleV1,
        after: Option<StreamOffsetV1>,
        through: Option<StreamOffsetV1>,
    ) -> Result<Vec<CommittedProducerStreamEventV1>, DurableStreamProducerError> {
        self.validate_handle(handle).await?;
        for offset in [after, through].into_iter().flatten() {
            StreamOffsetV1::from_bytes(*offset.as_bytes())
                .map_err(|error| DurableStreamProducerError::InvalidOffset(error.to_string()))?;
            self.validate_cursor(handle.stream_id, Some(offset)).await?;
        }
        if after
            .zip(through)
            .is_some_and(|(after, through)| after > through)
        {
            return Err(DurableStreamProducerError::InvalidOffset(
                "catch-up end precedes its cursor".into(),
            ));
        }
        if after.is_some() && after == through {
            return Ok(Vec::new());
        }
        if let Some(events) = self.retained_segment(handle.stream_id, after, through) {
            return Ok(events);
        }
        let (mut sequence, sequence_limit, terminal) = {
            let index = self
                .index_for([ProducerMetadataKey::Stream(handle.stream_id)])
                .await?;
            let stream = &index.streams[&handle.stream_id];
            (
                stream.first_sequence.unwrap_or(0),
                stream.next_sequence,
                stream.terminal,
            )
        };
        let mut enclosing_batch = None;
        if let Some(after) = after {
            let index = self
                .index_for([
                    ProducerMetadataKey::Stream(handle.stream_id),
                    ProducerMetadataKey::Position(handle.stream_id, after.producer_oplog_index()),
                ])
                .await?;
            let stream = &index.streams[&handle.stream_id];
            if stream.terminal && stream.last_offset == Some(after) {
                return Ok(Vec::new());
            }
            let (first, count) = index
                .batch_positions
                .get(&(handle.stream_id, after.producer_oplog_index()))
                .copied()
                .ok_or(DurableStreamProducerError::CursorUnavailable)?;
            let cursor_sequence = first
                .checked_add(u64::from(after.sub_index()))
                .ok_or(DurableStreamProducerError::CounterOverflow)?;
            if u64::from(after.sub_index()) >= count {
                return Err(DurableStreamProducerError::CursorUnavailable);
            }
            let batch_end = first
                .checked_add(count)
                .ok_or(DurableStreamProducerError::CounterOverflow)?;
            sequence = cursor_sequence
                .checked_add(1)
                .ok_or(DurableStreamProducerError::CounterOverflow)?;
            enclosing_batch =
                (sequence < batch_end).then_some((after.producer_oplog_index(), first));
        }
        let mut result = Vec::new();
        let mut result_bytes = 0usize;
        let mut locators = BTreeMap::new();
        loop {
            if result.len() >= STREAM_SEGMENT_MAX_EVENTS {
                return Ok(result);
            }
            if sequence >= sequence_limit && !terminal {
                return Ok(result);
            }
            if sequence >= sequence_limit {
                let index = self.index_for_terminal([], handle.stream_id).await?;
                if let Some(event) = index.streams[&handle.stream_id].terminal_event.clone()
                    && after.is_none_or(|after| event.offset > after)
                    && through.is_none_or(|through| event.offset <= through)
                    && result.len() < STREAM_SEGMENT_MAX_EVENTS
                    && (result.is_empty()
                        || result_bytes.saturating_add(encoded_event_bytes(&event))
                            <= STREAM_SEGMENT_TARGET_BYTES)
                {
                    result.push(event);
                }
                return Ok(result);
            }
            let (oplog_index, batch_first) = if let Some(enclosing) = enclosing_batch.take() {
                enclosing
            } else {
                if !locators.contains_key(&sequence) {
                    let locator_frontier =
                        125usize.min(sequence_limit.saturating_sub(sequence) as usize);
                    if self.control_metadata_provider.get().is_some() {
                        let mut index = self
                            .index_for([ProducerMetadataKey::Stream(handle.stream_id)])
                            .await?;
                        // Replace the locator window without evicting its stream/session
                        // dependencies and loading the same window twice.
                        if !index.complete_for_deletion {
                            index.loaded_metadata.retain(|key| match key {
                                ProducerMetadataKey::Batch(stream, _)
                                | ProducerMetadataKey::Position(stream, _) => {
                                    *stream != handle.stream_id
                                }
                                _ => true,
                            });
                            index
                                .streams
                                .get_mut(&handle.stream_id)
                                .unwrap()
                                .batches
                                .clear();
                            index
                                .batch_positions
                                .retain(|(stream, _), _| *stream != handle.stream_id);
                        }
                    }
                    let mut metadata_keys = Vec::with_capacity(locator_frontier + 1);
                    metadata_keys.push(ProducerMetadataKey::Stream(handle.stream_id));
                    metadata_keys.extend((0..locator_frontier).filter_map(|distance| {
                        sequence.checked_add(distance as u64).map(|candidate| {
                            ProducerMetadataKey::Batch(handle.stream_id, candidate)
                        })
                    }));
                    let index = self.index_for(metadata_keys).await?;
                    let stream = index
                        .streams
                        .get(&handle.stream_id)
                        .ok_or(DurableStreamProducerError::UnknownStream(handle.stream_id))?;
                    locators.extend(
                        stream
                            .batches
                            .range(sequence..sequence.saturating_add(locator_frontier as u64))
                            .map(|(&first, &oplog_index)| (first, oplog_index)),
                    );
                }
                let oplog_index = locators.remove(&sequence).ok_or_else(|| {
                    DurableStreamProducerError::CorruptHistory(
                        "stream metadata is missing a batch locator".into(),
                    )
                })?;
                (oplog_index, sequence)
            };
            let record = self.read_item_batch(oplog_index).await?;
            if record.stream_id != handle.stream_id || record.first_sequence != batch_first {
                return Err(DurableStreamProducerError::CorruptHistory(
                    "stream batch locator identifies a different batch".into(),
                ));
            }
            let events = self
                .materialize_item_batch_range(
                    &record,
                    sequence,
                    STREAM_SEGMENT_MAX_EVENTS.saturating_sub(result.len()),
                )
                .await?;
            if sequence < record.first_sequence
                || sequence
                    >= record
                        .first_sequence
                        .saturating_add(record.offsets.len() as u64)
            {
                return Err(DurableStreamProducerError::CorruptHistory(
                    "stream batch does not contain the requested sequence".into(),
                ));
            }
            let mut advanced = false;
            for event in events {
                if event.producer_sequence != sequence {
                    return Err(DurableStreamProducerError::CorruptHistory(
                        "stream batch event sequence is not contiguous".into(),
                    ));
                }
                if through.is_some_and(|through| event.offset > through) {
                    return Ok(result);
                }
                let bytes = encoded_event_bytes(&event);
                if !result.is_empty()
                    && (result.len() >= STREAM_SEGMENT_MAX_EVENTS
                        || result_bytes.saturating_add(bytes) > STREAM_SEGMENT_TARGET_BYTES)
                {
                    return Ok(result);
                }
                sequence = event.producer_sequence + 1;
                advanced = true;
                result_bytes = result_bytes.saturating_add(bytes);
                result.push(event);
                if result
                    .last()
                    .is_some_and(|event| Some(event.offset) == through)
                {
                    return Ok(result);
                }
            }
            if !advanced {
                return Err(DurableStreamProducerError::CorruptHistory(
                    "stream batch locator did not advance the requested sequence".into(),
                ));
            }
        }
    }
}

#[async_trait]
impl AttachedStreamSegmentSource for DurableStreamProducer {
    async fn journal_lag_events(
        &self,
        handle: &DurableStreamHandleV1,
        after: Option<StreamOffsetV1>,
    ) -> Result<usize, DurableStreamProducerError> {
        DurableStreamProducer::journal_lag_events(self, handle, after).await
    }

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
        let index = self
            .index_for([ProducerMetadataKey::Attachment(
                attachment.attachment_id,
                attachment.stream_id,
                attachment.consumer_environment_id,
                attachment.consumer.clone(),
            )])
            .await?;
        index.validate_attachment_key(
            attachment,
            self.environment_id,
            &self.producer,
            self.producer_fingerprint,
        )?;
        let indexed_attachment = index
            .attachments
            .get(&attachment_slot(attachment))
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
        drop(index);
        self.read_segment(handle, after, through).await
    }

    async fn wait_for_attached_segment(
        &self,
        attachment: &StreamAttachmentKeyV1,
        handle: &DurableStreamHandleV1,
        now_millis: u64,
        after: Option<StreamOffsetV1>,
    ) -> Result<Vec<CommittedProducerStreamEventV1>, DurableStreamProducerError> {
        let started = std::time::Instant::now();
        let events = self
            .read_attached_segment(attachment, handle, now_millis, after, None)
            .await?;
        if !events.is_empty() {
            return Ok(events);
        }
        let bus = self.stream_bus(handle.stream_id).await?;
        let subscription = bus.subscribe().await?;
        let mut reader = DurableCatchUpReader {
            bus,
            subscription: Some(subscription),
            history_source: None,
            history: VecDeque::new(),
            join_high_water: None,
            last_delivered: after,
            terminal_delivered: false,
        };
        let events = self
            .read_attached_segment(attachment, handle, now_millis, after, None)
            .await?;
        if !events.is_empty() {
            return Ok(events);
        }
        if let Ok(event) =
            tokio::time::timeout(std::time::Duration::from_secs(1), reader.next()).await
        {
            event?;
        }
        // The attachment may have been renewed, finalized, or replaced while the long poll was
        // asleep. Re-read it authoritatively and drain the full available segment rather than
        // returning only the bus event that happened to wake this waiter.
        let now_millis = now_millis.saturating_add(started.elapsed().as_millis() as u64);
        self.read_attached_segment(attachment, handle, now_millis, after, None)
            .await
    }
}

pub(crate) struct DurableCatchUpReader {
    bus: Arc<DurableLiveStreamBus<CommittedProducerStreamEventV1>>,
    subscription: Option<DurableLiveStreamSubscription<CommittedProducerStreamEventV1>>,
    history_source: Option<(Arc<DurableStreamProducer>, DurableStreamHandleV1)>,
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
        self.bus.ensure_available()?;
        if self.history.is_empty() {
            if self.last_delivered >= self.join_high_water {
                self.history_source = None;
            }
            if let Some((source, handle)) = &self.history_source {
                self.history = source
                    .read_segment(handle, self.last_delivered, self.join_high_water)
                    .await?
                    .into();
                if self.history.is_empty() {
                    return Err(DurableStreamProducerError::CorruptHistory(
                        "stream replay ended before its captured high-water mark".into(),
                    ));
                }
            }
        }
        self.bus.ensure_available()?;
        if let Some(event) = self.history.pop_front() {
            let event = self.deliver(event)?;
            self.release_subscription_if_complete();
            return Ok(Some(event));
        }
        loop {
            let Some(subscription) = self.subscription.as_mut() else {
                return Err(DurableLiveStreamBusError::PublicationAborted.into());
            };
            let event = subscription.recv().await?;
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
            self.history_source = None;
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
        ConsumerJournalSummary, DurableCatchUpReader, DurableLiveStreamBus,
        DurableLiveStreamBusError, DurableStreamCommit, DurableStreamProducer,
        DurableStreamProducerError, ExternalAppendOutcomeV1, ExternalProducerV1,
        IndexedConsumerJournal, ProducerOutputRegistrationV1, ProducerOutputSourceV1,
        ProducerRegistrationRequestV1, ProducerStreamIndex, StreamAttachmentConsumerProbe,
        StreamAttachmentControl, StreamAttachmentStateV1, StreamSegmentSource, registration_record,
    };
    use crate::services::oplog::{
        CommitLevel, DurableStreamOplogRecord, Oplog, OplogAddReceipt, OplogReadSource,
        OrderedOplogStart, PendingUpload, RawDurableStreamSessionStatus, checked_range_end,
        exact_from_source, fail_stop,
    };
    use async_trait::async_trait;
    use futures::FutureExt;
    use golem_common::base_model::component::{ComponentId, ComponentRevision};
    use golem_common::base_model::durable_stream::{
        AttachmentId, AttemptId, DURABLE_STREAM_FORMAT_VERSION, ExternalProducerIdV1,
        InputStreamHighWaterV1, MAX_DURABLE_STREAM_ITEM_SIZE, MAX_DURABLE_STREAMS_PER_SESSION,
        MAX_LIVE_JOIN_BUFFER_SIZE, MAX_NEW_STREAM_HANDLES_PER_VALUE,
        MAX_PACKED_U8_STREAM_ITEM_SIZE, MAX_STREAM_VALUE_TRAVERSAL_DEPTH,
        PersistedStreamInvocationDescriptorV1, STREAM_ATTACHMENT_ABANDONED_PREPARE_MILLIS,
        STREAM_ATTACHMENT_LEASE_TTL_MILLIS, SessionStreamRoleV1, StartAttemptDescriptorV1,
        StreamAttachmentFinalizationReasonV1, StreamAttachmentKeyV1, StreamCancelReasonV1,
        StreamCancelRoleV1, StreamCascadeDependentResultV1, StreamConsumerDeletingRecordV1,
        StreamConsumerItemValueRecordV1, StreamEndResultV1, StreamId, StreamInvocationIdV1,
        StreamItemsPayloadV1, StreamItemsRecordV1, StreamOffsetV1, StreamRegistrationCoordinateV1,
        StreamRootKindV1, StreamSessionKeyV1, StreamSessionMappingRecordV1,
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
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use test_r::{test, timeout};
    use tokio::sync::{Barrier, Notify, oneshot};
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
        read_ranges: Mutex<Vec<(OplogIndex, u64)>>,
        point_reads: AtomicU64,
    }

    impl Debug for TestOplog {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.debug_struct("TestOplog").finish()
        }
    }

    impl TestOplog {
        pub(crate) fn take_read_ranges(&self) -> Vec<(OplogIndex, u64)> {
            std::mem::take(&mut *self.read_ranges.lock().unwrap())
        }

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

        async fn raw_durable_stream_session_status(
            &self,
            session_key: &StreamInvocationIdV1,
        ) -> RawDurableStreamSessionStatus {
            let state = self.state.lock().unwrap();
            let watermark = state
                .entries
                .last_key_value()
                .map_or(OplogIndex::NONE, |(i, _)| *i);
            let mut status = golem_common::model::DurableStreamSessionStatus {
                session_key: Some(session_key.clone()),
                ..Default::default()
            };
            for (index, entry) in &state.entries {
                if let OplogEntry::PendingAgentInvocation {
                    idempotency_key, ..
                } = entry
                {
                    status.apply_pending_invocation(*index, idempotency_key);
                }
                if let OplogEntry::StreamSession { record, .. } = entry {
                    let record = match record {
                        OplogPayload::Inline(record) => record.as_ref(),
                        OplogPayload::SerializedInline {
                            cached: Some(record),
                            ..
                        }
                        | OplogPayload::External {
                            cached: Some(record),
                            ..
                        } => record.as_ref(),
                        _ => continue,
                    };
                    status.apply_record(*index, record);
                }
            }
            RawDurableStreamSessionStatus {
                watermark,
                status: Ok(Some(status)),
            }
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
            self.point_reads.fetch_add(1, Ordering::Relaxed);
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
            self.read_ranges.lock().unwrap().push((oplog_index, n));
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

    #[test]
    async fn nested_mutation_preserves_unfinished_parent_effects() {
        for parent_pending in [false, true] {
            for child_succeeds in [false, true] {
                for child_finishes in [false, true] {
                    let identity = identity();
                    let producer = DurableStreamProducer::load(
                        Arc::new(TestOplog::default()),
                        identity.environment_id,
                        identity.agent_id,
                        identity.fingerprint,
                        None,
                    )
                    .await
                    .unwrap();
                    let outcome: Result<(), DurableStreamProducerError> = producer
                        .run_owned(0, move |parent| async move {
                            if parent_pending {
                                parent.begin_durable_effect();
                            }
                            let child_result: Result<(), DurableStreamProducerError> = parent
                                .run_lifecycle(0, move |child| async move {
                                    child.begin_durable_effect();
                                    if child_finishes {
                                        child.finish_durable_effect();
                                    }
                                    if child_succeeds {
                                        Ok(())
                                    } else {
                                        Err(DurableStreamProducerError::ItemTooLarge)
                                    }
                                })
                                .await;
                            assert_eq!(child_result.is_ok(), child_succeeds);
                            Err(DurableStreamProducerError::ItemTooLarge)
                        })
                        .await;
                    assert!(outcome.is_err());
                    let requires_recovery = parent_pending || (!child_succeeds && !child_finishes);
                    assert_eq!(
                        producer.ensure_healthy().is_err(),
                        requires_recovery,
                        "parent_pending={parent_pending}, child_succeeds={child_succeeds}, child_finishes={child_finishes}"
                    );
                }
            }
        }
    }

    #[test]
    #[test_r::timeout("10s")]
    async fn nested_sibling_success_cannot_hide_failed_or_cancelled_effects() {
        for cancel_child in [false, true] {
            let identity = identity();
            let producer = DurableStreamProducer::load(
                Arc::new(TestOplog::default()),
                identity.environment_id,
                identity.agent_id,
                identity.fingerprint,
                None,
            )
            .await
            .unwrap();
            producer
                .run_owned(0, move |parent| async move {
                    let (release, released) = oneshot::channel();
                    let mut child = Box::pin(parent.run_owned(0, move |child| async move {
                        child.begin_durable_effect();
                        released.await.unwrap();
                        Err::<(), _>(DurableStreamProducerError::ItemTooLarge)
                    }));
                    assert!(futures::poll!(child.as_mut()).is_pending());
                    parent
                        .run_owned(0, |sibling| async move {
                            sibling.begin_durable_effect();
                            sibling.finish_durable_effect();
                            Ok::<(), DurableStreamProducerError>(())
                        })
                        .await?;
                    assert_eq!(parent.ensure_healthy(), Ok(()));
                    if cancel_child {
                        drop(child);
                    } else {
                        release.send(()).unwrap();
                        assert!(child.await.is_err());
                    }
                    Ok::<(), DurableStreamProducerError>(())
                })
                .await
                .unwrap();
            assert_eq!(
                producer.ensure_healthy(),
                Err(DurableStreamProducerError::RecoveryRequired)
            );
        }
    }

    #[test]
    async fn quiescent_retirement_rejects_storage_activity_without_poisoning_the_producer() {
        let identity = identity();
        let producer = DurableStreamProducer::load(
            Arc::new(TestOplog::default()),
            identity.environment_id,
            identity.agent_id,
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let activity = producer.durable_activity.try_enter().unwrap();
        assert!(!producer.try_retire_quiescent());
        assert_eq!(producer.ensure_healthy(), Ok(()));
        assert!(producer.durable_activity.try_enter().is_some());
        drop(activity);
        assert!(producer.try_retire_quiescent());
        assert_eq!(
            producer.ensure_healthy(),
            Err(DurableStreamProducerError::RecoveryRequired)
        );
        assert!(producer.durable_activity.try_enter().is_none());
    }

    #[test]
    #[test_r::timeout("10s")]
    async fn metadata_lookup_tracks_detached_storage_after_caller_cancellation() {
        for cancel_caller in [false, true] {
            let identity = identity();
            let producer = DurableStreamProducer::load(
                Arc::new(TestOplog::default()),
                identity.environment_id,
                identity.agent_id,
                identity.fingerprint,
                None,
            )
            .await
            .unwrap();
            let (release, released) = oneshot::channel();
            let (started, ready) = oneshot::channel();
            let mut lookup = Box::pin(producer.with_metadata_activity(async {
                crate::services::activity::spawn_with_activity(async move {
                    started.send(()).unwrap();
                    released.await.unwrap();
                })
                .await
                .unwrap();
                37
            }));
            assert!(futures::poll!(lookup.as_mut()).is_pending());
            ready.await.unwrap();
            assert!(!producer.try_retire_quiescent());
            producer.poison();
            let mut drain = Box::pin(producer.wait_durable_drained());
            if cancel_caller {
                drop(lookup);
                assert!(futures::poll!(drain.as_mut()).is_pending());
                release.send(()).unwrap();
            } else {
                assert!(futures::poll!(drain.as_mut()).is_pending());
                release.send(()).unwrap();
                assert_eq!(
                    lookup.await,
                    Err(DurableStreamProducerError::RecoveryRequired)
                );
            }
            drain.await;
            assert_eq!(
                producer
                    .with_metadata_activity(async { panic!("retired lookup ran") })
                    .await,
                Err::<(), _>(DurableStreamProducerError::RecoveryRequired)
            );
        }
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
            entity_parent_start_index: None,
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

    pub(crate) fn attachment_key(
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

    fn consumer_item_record(
        session_key: StreamSessionKeyV1,
        stream_id: StreamId,
        source_offset: StreamOffsetV1,
        consumer_read_ordinal: u64,
        value: Vec<u8>,
        packed_u8: bool,
    ) -> StreamSessionRecordV1 {
        StreamSessionRecordV1::ConsumerItemValue(StreamConsumerItemValueRecordV1 {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            session_key,
            stream_id,
            source_offset,
            consumer_read_ordinal,
            value,
            packed_u8,
            recursive_handles: Vec::new(),
            recursive_mappings: Vec::new(),
        })
    }

    #[test]
    fn consumer_journal_rejects_malformed_records_without_partial_mutation() {
        let identity = identity();
        let stream_id = StreamId(Uuid::from_u128(90));
        let key = (identity.invocation.clone(), stream_id);
        let mut index = ProducerStreamIndex::default();
        let empty_packed = consumer_item_record(
            identity.invocation.clone(),
            stream_id,
            StreamOffsetV1::new(OplogIndex::INITIAL, 0),
            0,
            Vec::new(),
            true,
        );
        assert!(matches!(
            index.apply_consumer_journal_record(&empty_packed),
            Err(DurableStreamProducerError::CorruptHistory(_))
        ));
        assert!(!index.consumer_journals.contains_key(&key));

        let overflowing_range = consumer_item_record(
            identity.invocation,
            stream_id,
            StreamOffsetV1::new(OplogIndex::INITIAL, u32::MAX),
            0,
            vec![1, 2],
            true,
        );
        assert!(matches!(
            index.apply_consumer_journal_record(&overflowing_range),
            Err(DurableStreamProducerError::CorruptHistory(_))
        ));
        assert!(!index.consumer_journals.contains_key(&key));

        index.consumer_journals.insert(
            key.clone(),
            IndexedConsumerJournal {
                next_read_ordinal: u64::MAX,
                ..Default::default()
            },
        );
        let ordinal_overflow = consumer_item_record(
            key.0.clone(),
            stream_id,
            StreamOffsetV1::new(OplogIndex::INITIAL, 0),
            u64::MAX,
            vec![1],
            false,
        );
        assert_eq!(
            index.apply_consumer_journal_record(&ordinal_overflow),
            Err(DurableStreamProducerError::CounterOverflow)
        );
        assert_eq!(index.consumer_journals[&key].next_read_ordinal, u64::MAX);
        assert_eq!(index.consumer_journals[&key].last_source_offset, None);
    }

    #[test]
    fn consumer_journal_validates_terminal_order_and_duplicate_overlay_ordinal() {
        let identity = identity();
        let stream_id = StreamId(Uuid::from_u128(91));
        let attachment = attachment_key(&identity, stream_id);
        let offset = StreamOffsetV1::new(OplogIndex::INITIAL, 0);
        let overlay = StreamSessionRecordV1::SourceUnavailable(
            golem_common::model::durable_stream::StreamSourceUnavailableRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                key: attachment.clone(),
                source_offset: offset,
                consumer_read_ordinal: 0,
            },
        );
        let mut index = ProducerStreamIndex::default();
        index.apply_consumer_journal_record(&overlay).unwrap();
        index.apply_consumer_journal_record(&overlay).unwrap();
        let mut wrong_ordinal = overlay.clone();
        let StreamSessionRecordV1::SourceUnavailable(record) = &mut wrong_ordinal else {
            unreachable!()
        };
        record.consumer_read_ordinal = 1;
        assert!(matches!(
            index.apply_consumer_journal_record(&wrong_ordinal),
            Err(DurableStreamProducerError::AttachmentConflict)
        ));

        let item =
            consumer_item_record(attachment.session_key, stream_id, offset, 0, vec![1], false);
        assert_eq!(
            index.apply_consumer_journal_record(&item),
            Err(DurableStreamProducerError::ConsumerJournalAdvanced)
        );
    }

    #[test]
    async fn consumer_source_unavailable_uses_the_warm_consumer_head_only() {
        let source_identity = identity();
        let stream_id = StreamId(Uuid::from_u128(92));
        let key = attachment_key(&source_identity, stream_id);
        let oplog = Arc::new(TestOplog::default());
        let consumer = DurableStreamProducer::load(
            oplog.clone(),
            key.consumer_environment_id,
            key.consumer.clone(),
            key.expected_consumer_fingerprint,
            None,
        )
        .await
        .unwrap();
        let offset = StreamOffsetV1::new(OplogIndex::from_u64(7), 3);
        consumer
            .append_session_record(StreamSessionRecordV1::SourceUnavailable(
                golem_common::model::durable_stream::StreamSourceUnavailableRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    key: key.clone(),
                    source_offset: offset,
                    consumer_read_ordinal: 0,
                },
            ))
            .await
            .unwrap();
        oplog.take_read_ranges();

        for _ in 0..3 {
            assert_eq!(
                consumer.consumer_source_unavailable(&key).await.unwrap(),
                Some(offset)
            );
        }
        assert!(oplog.take_read_ranges().is_empty());

        let mut wrong_identity = key;
        wrong_identity.expected_consumer_fingerprint = AgentFingerprint(Uuid::from_u128(999));
        assert!(matches!(
            consumer.consumer_source_unavailable(&wrong_identity).await,
            Err(DurableStreamProducerError::InvalidAttachmentState)
        ));
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

        async fn journal_summary(
            &self,
            _key: &StreamAttachmentKeyV1,
        ) -> Result<Option<ConsumerJournalSummary>, DurableStreamProducerError> {
            let inspection = self.inspection.lock().unwrap();
            Ok(Some(ConsumerJournalSummary {
                event_count: inspection.source_offsets.len() as u64,
                last_offset: inspection.source_offsets.last().copied(),
                terminal: true,
                source_unavailable: inspection.source_unavailable.is_some(),
            }))
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
    async fn delayed_stream_records_retain_registration_entity_attribution() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live = producer(oplog.clone(), &identity, None).await;
        let entity_parent_start_index = Some(OplogIndex::from_u64(42));
        let mut request = root_registration(&identity);
        request.entity_parent_start_index = entity_parent_start_index;
        let handle = live.register(request).await.unwrap().value;

        oplog.add(OplogEntry::no_op(None)).await;
        live.write_items(handle.stream_id, 0, StreamItemsPayloadV1::PackedU8(vec![1]))
            .await
            .unwrap();
        live.prepare_attachment(attachment_key(&identity, handle.stream_id), 100)
            .await
            .unwrap();
        live.end(handle.stream_id, 1, StreamEndResultV1::Ok)
            .await
            .unwrap();

        let attributed = oplog
            .entries()
            .into_iter()
            .filter(|entry| {
                matches!(
                    entry,
                    OplogEntry::StreamRegistered { .. }
                        | OplogEntry::StreamItems { .. }
                        | OplogEntry::StreamEnd { .. }
                        | OplogEntry::StreamSession { .. }
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(attributed.len(), 4);
        assert!(
            attributed
                .iter()
                .all(|entry| { entry.entity_parent_start_index() == entity_parent_start_index })
        );

        producer(oplog, &identity, None).await;
    }

    #[test]
    async fn item_payloads_are_loaded_only_for_the_requested_batch() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog.clone(), &identity, None).await;
        let handle = producer
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value;
        let payload = StreamItemsPayloadV1::PackedU8(vec![7; 64]);
        let first = producer
            .write_items(handle.stream_id, 0, payload.clone())
            .await
            .unwrap()
            .value;
        for sequence in (64..4096).step_by(64) {
            producer
                .write_items(handle.stream_id, sequence, payload.clone())
                .await
                .unwrap();
        }
        for _ in 0..2050 {
            oplog.add(OplogEntry::interrupted()).await;
        }
        assert!(
            producer.index.lock().await.streams[&handle.stream_id]
                .terminal_event
                .is_none()
        );
        oplog.take_read_ranges();
        let before = oplog.point_reads.load(Ordering::Relaxed);
        let events = producer
            .read_segment(&handle, Some(first[2]), Some(first[4]))
            .await
            .unwrap();
        assert_eq!(
            events.iter().map(|event| event.offset).collect::<Vec<_>>(),
            first[3..5]
        );
        assert_eq!(oplog.point_reads.load(Ordering::Relaxed) - before, 0);
        assert!(oplog.take_read_ranges().is_empty());
        assert!(
            producer
                .write_items(handle.stream_id, 0, payload)
                .await
                .unwrap()
                .replayed
        );
        assert!(matches!(
            producer
                .write_items(
                    handle.stream_id,
                    0,
                    StreamItemsPayloadV1::PackedU8(vec![8; 64])
                )
                .await,
            Err(DurableStreamProducerError::EventConflict)
        ));
    }

    #[test]
    async fn packed_retention_is_compact_and_materializes_only_a_bounded_partial_window() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog.clone(), &identity, None).await;
        let handle = producer
            .register(root_registration(&identity))
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

        {
            let retained = producer.committed_retention.lock().unwrap();
            assert_eq!(retained.batches.len(), 1);
            assert_eq!(retained.entries, 1);
            let super::RetainedCommittedEvents::Packed {
                first_offset,
                bytes,
                ..
            } = &retained.batches.front().unwrap().events
            else {
                panic!("packed write was expanded in retention")
            };
            assert_eq!(*first_offset, offsets[0]);
            assert_eq!(bytes.len(), offsets.len());
            assert!(retained.bytes < offsets.len() * 2);
        }
        let before = oplog.point_reads.load(Ordering::Relaxed);
        let events = producer
            .read_segment(&handle, Some(offsets[9]), None)
            .await
            .unwrap();
        assert_eq!(events.len(), super::STREAM_SEGMENT_MAX_EVENTS);
        assert_eq!(events[0].offset, offsets[10]);
        assert!(
            events
                .iter()
                .all(|event| event.packed_u8_batch_end == offsets.last().copied())
        );
        assert!(events.capacity() <= super::STREAM_SEGMENT_MAX_EVENTS);
        assert_eq!(oplog.point_reads.load(Ordering::Relaxed), before);
    }

    #[test]
    async fn retention_entry_budget_evicts_batches_instead_of_packed_items() {
        let identity = identity();
        let producer = producer(Arc::new(TestOplog::default()), &identity, None).await;
        let handle = producer
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value;
        for sequence in 0..=super::COMMITTED_RETENTION_MAX_ENTRIES as u64 {
            let event = CommittedProducerStreamEventV1 {
                stream_id: handle.stream_id,
                producer_sequence: sequence,
                offset: StreamOffsetV1::new(OplogIndex::from_u64(sequence + 1), 0),
                packed_u8_batch_end: None,
                terminal_author: None,
                nested_handles: Vec::new(),
                payload: CommittedProducerStreamEventPayloadV1::Value(vec![1]),
            };
            producer.retain_committed_events(&[event]);
        }
        let retained = producer.committed_retention.lock().unwrap();
        assert_eq!(retained.entries, super::COMMITTED_RETENTION_MAX_ENTRIES);
        assert_eq!(
            retained.batches.len(),
            super::COMMITTED_RETENTION_MAX_ENTRIES
        );
        assert!(
            retained
                .batches
                .iter()
                .all(|batch| batch.first_sequence() != 0)
        );
        assert!(
            retained.batches.iter().any(
                |batch| batch.first_sequence() == super::COMMITTED_RETENTION_MAX_ENTRIES as u64
            )
        );
    }

    #[test]
    async fn retained_segment_returns_prefix_at_encoded_byte_bound() {
        let identity = identity();
        let producer = producer(Arc::new(TestOplog::default()), &identity, None).await;
        let handle = producer
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value;
        for sequence in 0..2 {
            let event = CommittedProducerStreamEventV1 {
                stream_id: handle.stream_id,
                producer_sequence: sequence,
                offset: StreamOffsetV1::new(OplogIndex::from_u64(sequence + 1), 0),
                packed_u8_batch_end: None,
                terminal_author: None,
                nested_handles: Vec::new(),
                payload: CommittedProducerStreamEventPayloadV1::Value(vec![
                    0;
                    super::STREAM_SEGMENT_TARGET_BYTES
                        / 2
                        + 1
                ]),
            };
            producer.retain_committed_events(&[event]);
        }
        let events = producer
            .retained_segment(handle.stream_id, None, None)
            .expect("contiguous retained prefix");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].producer_sequence, 0);
    }

    #[test]
    async fn retention_gap_falls_back_to_authoritative_history() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog.clone(), &identity, None).await;
        let handle = producer
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value;
        let mut offsets = Vec::new();
        for sequence in 0..3 {
            offsets.extend(
                producer
                    .write_items(
                        handle.stream_id,
                        sequence,
                        StreamItemsPayloadV1::Values(vec![vec![sequence as u8]]),
                    )
                    .await
                    .unwrap()
                    .value,
            );
        }
        {
            let mut retained = producer.committed_retention.lock().unwrap();
            let missing = retained.batches.remove(1).unwrap();
            retained.entries -= 1;
            retained.bytes -= missing.retained_bytes;
        }
        let before = oplog.point_reads.load(Ordering::Relaxed);
        let events = producer
            .read_segment(&handle, Some(offsets[0]), None)
            .await
            .unwrap();
        assert_eq!(
            events.iter().map(|event| event.offset).collect::<Vec<_>>(),
            offsets[1..]
        );
        assert!(oplog.point_reads.load(Ordering::Relaxed) > before);
    }

    #[test]
    async fn cursor_validation_point_reads_without_historical_event_cache() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog.clone(), &identity, None).await;
        let handle = producer
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value;
        let offsets = producer
            .write_items(
                handle.stream_id,
                0,
                StreamItemsPayloadV1::PackedU8(vec![1, 2, 3]),
            )
            .await
            .unwrap()
            .value;
        for _ in 0..2050 {
            oplog.add(OplogEntry::interrupted()).await;
        }
        producer
            .end(handle.stream_id, 3, StreamEndResultV1::Ok)
            .await
            .unwrap();
        let high_water = producer.input_high_water(handle.stream_id).await.unwrap();
        {
            let mut index = producer.index.lock().await;
            let stream = index.streams.get_mut(&handle.stream_id).unwrap();
            stream.terminal_event = None;
            stream.batches.clear();
        }
        assert_eq!(
            producer.input_high_water(handle.stream_id).await.unwrap(),
            high_water
        );
        oplog.take_read_ranges();
        let before = oplog.point_reads.load(Ordering::Relaxed);
        producer
            .validate_cursor(handle.stream_id, Some(offsets[1]))
            .await
            .unwrap();
        assert_eq!(oplog.point_reads.load(Ordering::Relaxed) - before, 0);
        assert!(oplog.take_read_ranges().is_empty());
        assert!(
            producer
                .validate_cursor(
                    handle.stream_id,
                    Some(StreamOffsetV1::new(offsets[0].producer_oplog_index(), 3)),
                )
                .await
                .is_err()
        );
        assert!(
            producer
                .validate_cursor(
                    handle.stream_id,
                    Some(StreamOffsetV1::new(OplogIndex::NONE, 0)),
                )
                .await
                .is_err()
        );
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
    async fn attachment_slots_are_isolated_by_consumer_identity() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live = producer(oplog.clone(), &identity, None).await;
        let handle = live
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value;
        let first = attachment_key(&identity, handle.stream_id);
        let mut second = first.clone();
        second.consumer_environment_id = EnvironmentId(Uuid::from_u128(21));
        second.consumer = AgentId {
            component_id: ComponentId(Uuid::from_u128(22)),
            agent_id: "second-consumer".to_string(),
        };
        second.expected_consumer_fingerprint = AgentFingerprint(Uuid::from_u128(23));
        second.consumer_invocation.callee_environment_id = second.consumer_environment_id;
        second.consumer_invocation.callee = second.consumer.clone();
        second.consumer_invocation.callee_fingerprint = second.expected_consumer_fingerprint;

        for key in [&first, &second] {
            assert!(
                !live
                    .prepare_attachment(key.clone(), 100)
                    .await
                    .unwrap()
                    .replayed
            );
            assert!(
                live.prepare_attachment(key.clone(), 101)
                    .await
                    .unwrap()
                    .replayed
            );
            assert!(
                !live
                    .activate_attachment(key.clone(), 110)
                    .await
                    .unwrap()
                    .replayed
            );
            assert!(
                live.activate_attachment(key.clone(), 111)
                    .await
                    .unwrap()
                    .replayed
            );
        }

        let mut invalid_identity = first.clone();
        invalid_identity.expected_consumer_fingerprint = AgentFingerprint(Uuid::from_u128(99));
        invalid_identity.consumer_invocation.callee_fingerprint =
            invalid_identity.expected_consumer_fingerprint;
        assert_eq!(
            live.prepare_attachment(invalid_identity, 120).await,
            Err(DurableStreamProducerError::InvalidAttachmentState)
        );
        let mut invalid_epoch = second.clone();
        invalid_epoch.epoch = 2;
        assert_eq!(
            live.activate_attachment(invalid_epoch, 120).await,
            Err(DurableStreamProducerError::InvalidEpoch {
                current: 1,
                actual: 2,
            })
        );

        live.finalize_attachment(
            first.clone(),
            StreamAttachmentFinalizationReasonV1::ConsumerFinalized,
            130,
        )
        .await
        .unwrap();
        assert!(
            live.read_attached_segment(&second, &handle, 131, None, None)
                .await
                .unwrap()
                .is_empty()
        );
        drop(live);

        let restarted = producer(oplog, &identity, None).await;
        let attachments = restarted.inspect_attachments().await;
        assert_eq!(attachments.len(), 2);
        assert_eq!(attachments[0].key, first);
        assert_eq!(
            attachments[0].state,
            StreamAttachmentStateV1::Finalized(
                StreamAttachmentFinalizationReasonV1::ConsumerFinalized
            )
        );
        assert_eq!(attachments[1].key, second);
        assert_eq!(attachments[1].state, StreamAttachmentStateV1::Active);
        assert!(
            restarted
                .activate_attachment(second, 140)
                .await
                .unwrap()
                .replayed
        );
    }

    #[test]
    async fn active_attachment_count_spans_distinct_consumer_slots() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live = producer(oplog, &identity, None).await;
        let handle = live
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value;
        let mut first = attachment_key(&identity, handle.stream_id);
        first.session_key = identity.invocation.clone();
        first.attachment_id = AttachmentId::primary(
            first.session_key.callee_environment_id,
            &first.session_key.callee,
            &first.session_key.idempotency_key,
        )
        .unwrap();
        let mut second = first.clone();
        second.consumer_environment_id = EnvironmentId(Uuid::from_u128(21));
        second.consumer = AgentId {
            component_id: ComponentId(Uuid::from_u128(22)),
            agent_id: "second-consumer".to_string(),
        };
        second.expected_consumer_fingerprint = AgentFingerprint(Uuid::from_u128(23));
        second.consumer_invocation.callee_environment_id = second.consumer_environment_id;
        second.consumer_invocation.callee = second.consumer.clone();
        second.consumer_invocation.callee_fingerprint = second.expected_consumer_fingerprint;

        live.prepare_attachment(first.clone(), 100).await.unwrap();
        assert!(
            !live
                .has_active_attachment(&first.session_key, &handle)
                .await
                .unwrap()
        );
        live.activate_attachment(first.clone(), 110).await.unwrap();
        live.prepare_attachment(second.clone(), 100).await.unwrap();
        live.activate_attachment(second.clone(), 110).await.unwrap();
        assert!(
            live.has_active_attachment(&first.session_key, &handle)
                .await
                .unwrap()
        );
        live.finalize_attachment(
            first.clone(),
            StreamAttachmentFinalizationReasonV1::ConsumerFinalized,
            120,
        )
        .await
        .unwrap();
        assert!(
            live.has_active_attachment(&first.session_key, &handle)
                .await
                .unwrap()
        );
        live.finalize_attachment(
            second,
            StreamAttachmentFinalizationReasonV1::ConsumerFinalized,
            120,
        )
        .await
        .unwrap();
        assert!(
            !live
                .has_active_attachment(&first.session_key, &handle)
                .await
                .unwrap()
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
                entity_parent_start_index: None,
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
            restarted
                .deletion_diagnostics()
                .await
                .unwrap()
                .cascade_completed,
            vec![(
                key,
                StreamCascadeDependentResultV1::SourceUnavailable {
                    first_unjournaled_offset: first_offset,
                },
            )]
        );
    }

    #[test]
    #[timeout("30s")]
    async fn deletion_cascade_does_not_wait_for_a_stalled_live_reader() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live = producer(oplog.clone(), &identity, Some(1)).await;
        let handle = live
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value;
        let mut reader = live
            .bus(handle.stream_id)
            .unwrap()
            .subscribe()
            .await
            .unwrap();
        let first_offset = live
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

        tokio::time::timeout(Duration::from_secs(5), live.cascade_deletion(200, &probe))
            .await
            .expect("deletion waited for live delivery instead of durability")
            .unwrap();
        assert_eq!(probe.overlay_commits.load(Ordering::Relaxed), 1);
        assert_eq!(
            probe.inspection.lock().unwrap().source_unavailable,
            Some(first_offset)
        );
        let restarted = producer(oplog, &identity, None).await;
        assert_eq!(
            restarted
                .deletion_diagnostics()
                .await
                .unwrap()
                .cascade_completed,
            vec![(
                key,
                StreamCascadeDependentResultV1::SourceUnavailable {
                    first_unjournaled_offset: first_offset,
                }
            )]
        );
        assert_eq!(reader.recv().await.unwrap().offset, first_offset);
        let terminal = reader.recv().await.unwrap();
        assert!(terminal.offset > first_offset);
        assert!(terminal.payload.is_terminal());
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

        let diagnostics = live.deletion_diagnostics().await.unwrap();
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
        assert_eq!(restarted.deletion_diagnostics().await.unwrap(), diagnostics);
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
                .unwrap()
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
            live.deletion_diagnostics().await.unwrap().cascade_completed,
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
                .unwrap()
                .cascade_completed
                .is_empty()
        );
        live.cascade_deletion(201, &probe).await.unwrap();
        assert_eq!(probe.commits.load(Ordering::Relaxed), 1);
        assert_eq!(
            live.deletion_diagnostics().await.unwrap().cascade_completed,
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
                entity_parent_start_index: None,
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
    #[test_r::timeout("30s")]
    async fn closed_export_reports_terminal_state_before_final_page() {
        use golem_common::model::durable_stream::StreamHandleReadRequestV1;
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
                StreamItemsPayloadV1::PackedU8(vec![3, 9, 17, 41]),
            )
            .await
            .unwrap()
            .value;
        live.end(handle.stream_id, 4, StreamEndResultV1::Ok)
            .await
            .unwrap();
        let read = live
            .read_by_handle(StreamHandleReadRequestV1 {
                handle,
                after: None,
                max_items: 2,
                max_bytes: 10,
                wait_millis: 0,
            })
            .await
            .unwrap();
        assert!(read.closed);
        assert!(!read.cancelled);
        assert_eq!(read.events.len(), 2);
        assert_eq!(read.next_offset, Some(offsets[1]));
        assert!(read.next_offset < read.head_offset);
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn blocked_terminal_publication_wakes_export_reader_after_durable_commit() {
        use golem_common::model::durable_stream::StreamHandleReadRequestV1;
        let identity = identity();
        let live = producer(Arc::new(TestOplog::default()), &identity, Some(1)).await;
        let handle = live
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value;
        let bus = live.stream_bus(handle.stream_id).await.unwrap();
        let mut reader = bus.subscribe().await.unwrap();
        let offset = live
            .write_items(handle.stream_id, 0, StreamItemsPayloadV1::PackedU8(vec![3]))
            .await
            .unwrap()
            .value[0];

        let export = live.read_by_handle(StreamHandleReadRequestV1 {
            handle: handle.clone(),
            after: Some(offset),
            max_items: 2,
            max_bytes: 10,
            wait_millis: 5_000,
        });
        tokio::pin!(export);
        assert!(futures::poll!(&mut export).is_pending());

        let terminal = tokio::spawn({
            let live = live.clone();
            async move { live.end(handle.stream_id, 1, StreamEndResultV1::Ok).await }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let (_, closed, _) = live.stream_head(&handle).await.unwrap();
                if closed {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("terminal commit did not finish");
        assert!(
            !terminal.is_finished(),
            "publication must remain backpressured"
        );

        let read = tokio::time::timeout(std::time::Duration::from_millis(100), export)
            .await
            .expect("durably committed terminal must wake the export reader")
            .unwrap();
        assert!(read.closed);
        assert_eq!(read.events.len(), 1);
        reader.recv().await.unwrap();
        terminal.await.unwrap().unwrap();
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn unattached_slot_reads_paginate_and_wake_both_readers() {
        use golem_common::model::durable_stream::StreamHandleReadRequestV1;
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live = producer(oplog.clone(), &identity, None).await;
        let handle = live
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value;
        let key = attachment_key(&identity, handle.stream_id);
        live.prepare_attachment(key.clone(), 100).await.unwrap();
        live.activate_attachment(key.clone(), 101).await.unwrap();
        let offsets = live
            .write_items(
                handle.stream_id,
                0,
                StreamItemsPayloadV1::PackedU8(vec![3, 9, 17]),
            )
            .await
            .unwrap()
            .value;
        let request = StreamHandleReadRequestV1 {
            handle: handle.clone(),
            after: None,
            max_items: 2,
            max_bytes: 10,
            wait_millis: 0,
        };
        let first = live.read_by_handle(request.clone()).await.unwrap();
        assert_eq!(
            first
                .events
                .iter()
                .map(|event| event.payload.clone())
                .collect::<Vec<_>>(),
            vec![
                CommittedProducerStreamEventPayloadV1::PackedU8(3),
                CommittedProducerStreamEventPayloadV1::PackedU8(9)
            ]
        );
        assert_eq!(first.next_offset, Some(offsets[1]));
        assert_eq!(first.head_offset, Some(offsets[2]));
        let byte_limited = live
            .read_by_handle(StreamHandleReadRequestV1 {
                max_items: 20,
                max_bytes: 1,
                ..request.clone()
            })
            .await
            .unwrap();
        assert_eq!(byte_limited.events.len(), 1);
        assert_eq!(byte_limited.next_offset, Some(offsets[0]));
        let head = live
            .read_by_handle(StreamHandleReadRequestV1 {
                max_items: 0,
                max_bytes: 0,
                ..request.clone()
            })
            .await
            .unwrap();
        assert!(head.events.is_empty());
        assert_eq!(head.next_offset, None);
        assert_eq!(head.head_offset, Some(offsets[2]));
        for after in [
            offsets[0],
            offsets[2],
            StreamOffsetV1::new(OplogIndex::from_u64(999), 0),
        ] {
            let head = live
                .read_by_handle(StreamHandleReadRequestV1 {
                    after: Some(after),
                    max_items: 0,
                    max_bytes: 0,
                    ..request.clone()
                })
                .await
                .unwrap();
            assert_eq!(head.next_offset, Some(after));
            assert_eq!(head.head_offset, Some(offsets[2]));
            assert!(head.events.is_empty());
        }
        let wait = StreamHandleReadRequestV1 {
            after: Some(offsets[2]),
            wait_millis: 5_000,
            ..request.clone()
        };
        let bus = live.stream_bus(handle.stream_id).await.unwrap();
        let mut attached_readers = Vec::new();
        for _ in 0..golem_common::base_model::durable_stream::MAX_LIVE_READERS_PER_STREAM {
            attached_readers.push(bus.subscribe().await.unwrap());
        }
        let left = live.read_by_handle(wait.clone());
        let right = live.read_by_handle(wait);
        tokio::pin!(left, right);
        assert!(futures::poll!(&mut left).is_pending());
        assert!(futures::poll!(&mut right).is_pending());
        assert!(matches!(
            bus.subscribe().await,
            Err(super::DurableLiveStreamBusError::ReaderLimit)
        ));
        let next = live
            .write_items(
                handle.stream_id,
                3,
                StreamItemsPayloadV1::PackedU8(vec![41]),
            )
            .await
            .unwrap()
            .value[0];
        let (left, right) = tokio::join!(left, right);
        let left = left.unwrap();
        assert_eq!(left, right.unwrap());
        assert_eq!(left.events.len(), 1);
        assert_eq!(
            left.events[0].payload,
            CommittedProducerStreamEventPayloadV1::PackedU8(41)
        );
        assert_eq!(left.next_offset, Some(next));
        assert_eq!(live.index.lock().await.attachments.len(), 1);
        let attached = live
            .read_attached_segment(&key, &handle, 102, None, None)
            .await
            .unwrap();
        assert_eq!(attached.len(), 4);
        let beyond = StreamOffsetV1::new(
            OplogIndex::from_u64(next.producer_oplog_index().as_u64() + 100),
            0,
        );
        let result = live
            .read_by_handle(StreamHandleReadRequestV1 {
                after: Some(beyond),
                ..request
            })
            .await
            .unwrap();
        assert!(result.events.is_empty());
        assert_eq!(result.next_offset, Some(beyond));
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn future_cursor_live_reads_wait_for_deadline_or_closure() {
        use golem_common::model::durable_stream::StreamHandleReadRequestV1;
        let identity = identity();
        let live = producer(Arc::new(TestOplog::default()), &identity, None).await;
        let handle = live
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value;
        let request = StreamHandleReadRequestV1 {
            handle: handle.clone(),
            after: Some(StreamOffsetV1::new(OplogIndex::from_u64(999), 0)),
            max_items: 10,
            max_bytes: 100,
            wait_millis: 100,
        };
        let started = tokio::time::Instant::now();
        let timed_out = live.read_by_handle(request.clone()).await.unwrap();
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert!(timed_out.events.is_empty());
        assert_eq!(timed_out.next_offset, request.after);
        assert!(!timed_out.closed);

        let mut waiting = Box::pin(live.read_by_handle(StreamHandleReadRequestV1 {
            wait_millis: 5_000,
            ..request.clone()
        }));
        assert!(futures::poll!(waiting.as_mut()).is_pending());
        live.write_items(handle.stream_id, 0, StreamItemsPayloadV1::PackedU8(vec![7]))
            .await
            .unwrap();
        assert!(futures::poll!(waiting.as_mut()).is_pending());
        live.end(handle.stream_id, 1, StreamEndResultV1::Ok)
            .await
            .unwrap();
        let closed = waiting.await.unwrap();
        assert!(closed.closed);
        assert!(closed.events.is_empty());
        assert_eq!(closed.next_offset, request.after);
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn normal_and_external_batches_publish_in_commit_order_after_caller_abort() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live = producer(oplog, &identity, Some(1)).await;
        let handle = live
            .register(registration(
                &identity,
                StreamRegistrationCoordinateV1::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodInput,
                    recursive_value_path: vec![StreamValuePathStepV1::TupleElement(0)],
                },
                StreamSourceKindV1::ExternalInlineInput,
            ))
            .await
            .unwrap()
            .value;
        let bus = live.bus(handle.stream_id).unwrap();
        let mut reader = bus.subscribe().await.unwrap();
        let first = tokio::spawn({
            let live = live.clone();
            let id = handle.stream_id;
            async move {
                live.write_items(id, 0, StreamItemsPayloadV1::PackedU8(vec![3, 7, 19]))
                    .await
            }
        });
        while live.stream_head(&handle).await.unwrap().0.is_none() {
            tokio::task::yield_now().await;
        }
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        let second = live.append_external_input(
            &identity.invocation,
            handle.stream_id,
            Some(StreamItemsPayloadV1::PackedU8(vec![31, 44])),
            true,
            None,
        );
        let read = async {
            let mut events = Vec::new();
            for _ in 0..6 {
                events.push(reader.recv().await.unwrap());
            }
            events
        };
        let (accepted, events) = tokio::join!(second, read);
        assert_eq!(
            events
                .iter()
                .map(|event| event.payload.payload.clone())
                .collect::<Vec<_>>(),
            vec![
                CommittedProducerStreamEventPayloadV1::PackedU8(3),
                CommittedProducerStreamEventPayloadV1::PackedU8(7),
                CommittedProducerStreamEventPayloadV1::PackedU8(19),
                CommittedProducerStreamEventPayloadV1::PackedU8(31),
                CommittedProducerStreamEventPayloadV1::PackedU8(44),
                CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::Ok),
            ]
        );
        assert!(
            events
                .windows(2)
                .all(|pair| pair[0].offset < pair[1].offset)
        );
        assert_eq!(
            accepted.unwrap(),
            ExternalAppendOutcomeV1::Accepted(events[5].offset)
        );
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn failed_commit_callbacks_fence_cached_reads_and_recover_committed_items() {
        use golem_common::model::durable_stream::StreamHandleReadRequestV1;

        for fail_after_receipt in [false, true] {
            let identity = identity();
            let oplog = Arc::new(TestOplog::default());
            let fail = Arc::new(AtomicBool::new(false));
            let commit: DurableStreamCommit = Arc::new({
                let oplog = oplog.clone();
                let fail = fail.clone();
                move |receipt| {
                    let oplog = oplog.clone();
                    let fail = fail.clone();
                    Box::pin(async move {
                        oplog.commit(CommitLevel::Always).await;
                        assert!(
                            fail_after_receipt || !fail.load(Ordering::Acquire),
                            "injected failure before durability receipt"
                        );
                        if let Some(receipt) = receipt {
                            let _ = receipt.send(());
                        }
                        assert!(
                            !fail.load(Ordering::Acquire),
                            "injected failure after durability receipt"
                        );
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
            let handle = live
                .register(root_registration(&identity))
                .await
                .unwrap()
                .value;
            let payload = StreamItemsPayloadV1::PackedU8(vec![13, 79]);
            fail.store(true, Ordering::Release);
            assert!(
                live.write_items(handle.stream_id, 0, payload.clone())
                    .await
                    .is_err()
            );
            let request = StreamHandleReadRequestV1 {
                handle: handle.clone(),
                after: None,
                max_items: 16,
                max_bytes: 4096,
                wait_millis: 0,
            };
            assert!(
                live.read_by_handle(request.clone()).await.is_err(),
                "cached reads must not conceal an uncertain commit outcome"
            );
            assert!(
                live.write_items(handle.stream_id, 0, payload.clone())
                    .await
                    .is_err()
            );

            let recovered = DurableStreamProducer::load(
                oplog,
                identity.environment_id,
                identity.agent_id,
                identity.fingerprint,
                None,
            )
            .await
            .unwrap();
            let read = recovered.read_by_handle(request).await.unwrap();
            assert_eq!(
                read.events
                    .iter()
                    .map(|event| event.payload.clone())
                    .collect::<Vec<_>>(),
                vec![
                    CommittedProducerStreamEventPayloadV1::PackedU8(13),
                    CommittedProducerStreamEventPayloadV1::PackedU8(79)
                ]
            );
            let retry = recovered
                .write_items(handle.stream_id, 0, payload)
                .await
                .unwrap();
            assert!(retry.replayed);
            assert_eq!(
                retry.value,
                read.events
                    .iter()
                    .map(|event| event.offset)
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn handle_read_hydrates_cancellation_committed_before_request_abort() {
        use golem_common::model::durable_stream::StreamHandleReadRequestV1;

        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let block_commit = Arc::new(AtomicBool::new(false));
        let committed = Arc::new(Notify::new());
        let commit: DurableStreamCommit = Arc::new({
            let oplog = oplog.clone();
            let block_commit = block_commit.clone();
            let committed = committed.clone();
            move |published| {
                let oplog = oplog.clone();
                let block_commit = block_commit.clone();
                let committed = committed.clone();
                Box::pin(async move {
                    oplog.commit(CommitLevel::Always).await;
                    if let Some(published) = published {
                        let _ = published.send(());
                    }
                    if block_commit.load(Ordering::SeqCst) {
                        committed.notify_waiters();
                        futures::future::pending().await
                    }
                })
            }
        });
        let live = DurableStreamProducer::load_with_commit(
            oplog,
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            None,
            commit,
        )
        .await
        .unwrap();
        let handle = live
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value;

        block_commit.store(true, Ordering::SeqCst);
        let notification = committed.notified();
        let cancellation = tokio::spawn({
            let live = live.clone();
            async move {
                live.cancel_open(
                    handle.stream_id,
                    StreamCancelRoleV1::OutputConsumer,
                    StreamCancelReasonV1::Cancelled,
                    None,
                )
                .await
            }
        });
        notification.await;
        cancellation.abort();
        assert!(cancellation.await.unwrap_err().is_cancelled());

        let read = live
            .read_by_handle(StreamHandleReadRequestV1 {
                handle,
                after: None,
                max_items: 0,
                max_bytes: 0,
                wait_millis: 0,
            })
            .await
            .unwrap();
        assert!(
            read.closed,
            "durably committed cancellation was not hydrated"
        );
        assert!(
            read.cancelled,
            "durably committed cancellation was not hydrated"
        );
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn external_append_retry_after_commit_cancellation_is_duplicate() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let block_commit = Arc::new(AtomicBool::new(false));
        let committed = Arc::new(Notify::new());
        let commit: DurableStreamCommit = Arc::new({
            let oplog = oplog.clone();
            let block_commit = block_commit.clone();
            let committed = committed.clone();
            move |published| {
                let oplog = oplog.clone();
                let block_commit = block_commit.clone();
                let committed = committed.clone();
                Box::pin(async move {
                    oplog.commit(CommitLevel::Always).await;
                    if let Some(published) = published {
                        let _ = published.send(());
                    }
                    if block_commit.load(Ordering::SeqCst) {
                        committed.notify_waiters();
                        futures::future::pending().await
                    }
                })
            }
        });
        let live = DurableStreamProducer::load_with_commit(
            oplog,
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            None,
            commit,
        )
        .await
        .unwrap();
        let handle = live
            .register(registration(
                &identity,
                StreamRegistrationCoordinateV1::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodInput,
                    recursive_value_path: vec![StreamValuePathStepV1::TupleElement(0)],
                },
                StreamSourceKindV1::ExternalInlineInput,
            ))
            .await
            .unwrap()
            .value;
        let external = ExternalProducerV1 {
            id: ExternalProducerIdV1::Client("retrying-producer".into()),
            epoch: 1,
            sequence: 0,
        };

        block_commit.store(true, Ordering::SeqCst);
        let notification = committed.notified();
        let append = tokio::spawn({
            let live = live.clone();
            let session = identity.invocation.clone();
            let external = external.clone();
            async move {
                live.append_external_input(
                    &session,
                    handle.stream_id,
                    Some(StreamItemsPayloadV1::PackedU8(vec![7])),
                    false,
                    Some(external),
                )
                .await
            }
        });
        notification.await;
        append.abort();
        assert!(append.await.unwrap_err().is_cancelled());
        block_commit.store(false, Ordering::SeqCst);

        let retry = live
            .append_external_input(
                &identity.invocation,
                handle.stream_id,
                Some(StreamItemsPayloadV1::PackedU8(vec![7])),
                false,
                Some(external),
            )
            .await
            .unwrap();
        assert!(
            matches!(retry, ExternalAppendOutcomeV1::Duplicate { .. }),
            "durably committed producer sequence was appended again: {retry:?}"
        );
    }

    // Provisional bug-finder test: external input records must retain their stream owner.
    #[test]
    async fn provisional_external_append_retains_entity_attribution() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live = producer(oplog.clone(), &identity, None).await;
        let attribution = Some(OplogIndex::from_u64(71));
        let mut request = registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodInput,
                recursive_value_path: vec![StreamValuePathStepV1::TupleElement(0)],
            },
            StreamSourceKindV1::ExternalInlineInput,
        );
        request.entity_parent_start_index = attribution;
        let handle = live.register(request).await.unwrap().value;
        let before = oplog.current_oplog_index().await;

        live.append_external_input(
            &identity.invocation,
            handle.stream_id,
            Some(StreamItemsPayloadV1::PackedU8(vec![7])),
            true,
            Some(ExternalProducerV1 {
                id: ExternalProducerIdV1::Client("attributed-producer".into()),
                epoch: 1,
                sequence: 0,
            }),
        )
        .await
        .unwrap();

        let after = oplog.current_oplog_index().await;
        for position in before.next().as_u64()..=after.as_u64() {
            assert_eq!(
                oplog
                    .read(OplogIndex::from_u64(position))
                    .await
                    .entity_parent_start_index(),
                attribution
            );
        }
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn attached_input_distinguishes_foreign_close_from_its_own_end_after_reload() {
        for sent_item in [false, true] {
            for own_end in [false, true] {
                let identity = identity();
                let oplog = Arc::new(TestOplog::default());
                let live = producer(oplog.clone(), &identity, None).await;
                let handle = live
                    .register(registration(
                        &identity,
                        StreamRegistrationCoordinateV1::Root {
                            invocation_id: identity.invocation.clone(),
                            root_kind: StreamRootKindV1::MethodInput,
                            recursive_value_path: vec![StreamValuePathStepV1::TupleElement(0)],
                        },
                        StreamSourceKindV1::ExternalInlineInput,
                    ))
                    .await
                    .unwrap()
                    .value;
                if sent_item {
                    live.write_attached_items_with_nested(
                        &identity.invocation,
                        handle.stream_id,
                        0,
                        StreamItemsPayloadV1::PackedU8(vec![13]),
                        Vec::new(),
                    )
                    .await
                    .unwrap();
                }
                let end_sequence = u64::from(sent_item);
                live.append_external_input(
                    &identity.invocation,
                    handle.stream_id,
                    None,
                    true,
                    own_end.then_some(ExternalProducerV1 {
                        id: ExternalProducerIdV1::Attached,
                        epoch: 0,
                        sequence: end_sequence,
                    }),
                )
                .await
                .unwrap();
                drop(live);

                let recovered = producer(oplog.clone(), &identity, None).await;
                let tip = oplog.current_oplog_index().await;
                let error = recovered
                    .write_attached_items_with_nested(
                        &identity.invocation,
                        handle.stream_id,
                        end_sequence + u64::from(own_end),
                        StreamItemsPayloadV1::PackedU8(vec![17]),
                        Vec::new(),
                    )
                    .await
                    .unwrap_err();
                assert_eq!(
                    error,
                    if own_end {
                        DurableStreamProducerError::FencedByTerminal(
                            CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::Ok),
                        )
                    } else {
                        DurableStreamProducerError::ClosedByOtherProducer
                    }
                );
                if !own_end {
                    assert_eq!(
                        recovered
                            .write_attached_items_with_nested(
                                &identity.invocation,
                                handle.stream_id,
                                end_sequence + 2,
                                StreamItemsPayloadV1::PackedU8(vec![23]),
                                Vec::new(),
                            )
                            .await
                            .unwrap_err(),
                        DurableStreamProducerError::ClosedByOtherProducer,
                        "all frames already in flight must be discarded after a foreign close",
                    );
                }
                if own_end {
                    assert_eq!(
                        recovered
                            .write_attached_items_with_nested(
                                &identity.invocation,
                                handle.stream_id,
                                end_sequence,
                                StreamItemsPayloadV1::PackedU8(vec![19]),
                                Vec::new(),
                            )
                            .await
                            .unwrap_err(),
                        DurableStreamProducerError::EventConflict,
                    );
                }
                assert_eq!(oplog.current_oplog_index().await, tip);
            }
        }
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn attached_and_client_appends_interleave_and_reconstruct_multi_value_retry() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live = producer(oplog.clone(), &identity, None).await;
        let handle = live
            .register(registration(
                &identity,
                StreamRegistrationCoordinateV1::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodInput,
                    recursive_value_path: vec![StreamValuePathStepV1::TupleElement(98)],
                },
                StreamSourceKindV1::ExternalInlineInput,
            ))
            .await
            .unwrap()
            .value;

        live.write_attached_items_with_nested(
            &identity.invocation,
            handle.stream_id,
            0,
            StreamItemsPayloadV1::PackedU8(vec![10]),
            Vec::new(),
        )
        .await
        .unwrap();
        let client = ExternalProducerV1 {
            id: ExternalProducerIdV1::Client("multi".into()),
            epoch: 0,
            sequence: 0,
        };
        let accepted = live
            .append_external_input(
                &identity.invocation,
                handle.stream_id,
                Some(StreamItemsPayloadV1::Values(vec![vec![20], vec![30]])),
                false,
                Some(client.clone()),
            )
            .await
            .unwrap();
        live.write_attached_items_with_nested(
            &identity.invocation,
            handle.stream_id,
            1,
            StreamItemsPayloadV1::PackedU8(vec![40]),
            Vec::new(),
        )
        .await
        .unwrap();
        drop(live);

        let recovered = producer(oplog.clone(), &identity, None).await;
        let ExternalAppendOutcomeV1::Accepted(original_offset) = accepted else {
            panic!("client append was not accepted")
        };
        assert_eq!(
            recovered
                .append_external_input(
                    &identity.invocation,
                    handle.stream_id,
                    Some(StreamItemsPayloadV1::Values(vec![vec![20], vec![30]])),
                    false,
                    Some(client),
                )
                .await
                .unwrap(),
            ExternalAppendOutcomeV1::Duplicate {
                offset: original_offset,
                highest_sequence: Some(0),
            }
        );
        assert_eq!(
            recovered
                .attached_input_high_water(&identity.invocation, handle.stream_id)
                .await
                .unwrap()
                .unwrap()
                .highest_contiguous_sequence,
            1
        );
        let retry = recovered
            .write_attached_items_with_nested(
                &identity.invocation,
                handle.stream_id,
                1,
                StreamItemsPayloadV1::PackedU8(vec![40]),
                Vec::new(),
            )
            .await
            .unwrap();
        assert!(retry.replayed);
        assert!(matches!(
            recovered
                .write_attached_items_with_nested(
                    &identity.invocation,
                    handle.stream_id,
                    1,
                    StreamItemsPayloadV1::PackedU8(vec![41]),
                    Vec::new(),
                )
                .await,
            Err(DurableStreamProducerError::EventConflict)
        ));
        recovered
            .append_external_input(
                &identity.invocation,
                handle.stream_id,
                None,
                true,
                Some(ExternalProducerV1 {
                    id: ExternalProducerIdV1::Attached,
                    epoch: 0,
                    sequence: 2,
                }),
            )
            .await
            .unwrap();
        let events = recovered.read_segment(&handle, None, None).await.unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| event.producer_sequence)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3, 4]
        );
        assert!(matches!(
            events[0].payload,
            CommittedProducerStreamEventPayloadV1::PackedU8(10)
        ));
        assert_eq!(
            events[1].payload,
            CommittedProducerStreamEventPayloadV1::Value(vec![20])
        );
        assert_eq!(
            events[2].payload,
            CommittedProducerStreamEventPayloadV1::Value(vec![30])
        );
        assert!(matches!(
            events[3].payload,
            CommittedProducerStreamEventPayloadV1::PackedU8(40)
        ));
        assert!(events[4].is_terminal());

        let item_records = oplog
            .read_exact(
                OplogIndex::INITIAL,
                oplog.current_oplog_index().await.as_u64(),
            )
            .await
            .into_iter()
            .filter(|(_, entry)| matches!(entry, OplogEntry::StreamItems { .. }))
            .count();
        assert_eq!(
            item_records, 4,
            "each value append must have its own Items record"
        );
    }

    #[test]
    #[timeout("30s")]
    async fn attached_packed_and_nested_inputs_keep_transport_sequences_during_http_writes() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live = producer(oplog.clone(), &identity, None).await;
        let handle = live
            .register(registration(
                &identity,
                StreamRegistrationCoordinateV1::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodInput,
                    recursive_value_path: vec![StreamValuePathStepV1::TupleElement(97)],
                },
                StreamSourceKindV1::ExternalInlineInput,
            ))
            .await
            .unwrap()
            .value;
        let (attached, http) = tokio::join!(
            live.write_attached_items_with_nested(
                &identity.invocation,
                handle.stream_id,
                0,
                StreamItemsPayloadV1::PackedU8(vec![10, 11]),
                Vec::new(),
            ),
            live.append_external_input(
                &identity.invocation,
                handle.stream_id,
                Some(StreamItemsPayloadV1::Values(vec![
                    vec![20],
                    vec![30],
                    vec![31]
                ])),
                false,
                None,
            ),
        );
        assert_eq!(attached.unwrap().value.len(), 2);
        assert!(matches!(
            http.unwrap(),
            ExternalAppendOutcomeV1::Accepted(_)
        ));
        let nested = registration(
            &identity,
            StreamRegistrationCoordinateV1::Nested {
                parent_stream_id: handle.stream_id,
                parent_producer_sequence: 2,
                recursive_value_path: vec![StreamValuePathStepV1::TupleElement(0)],
            },
            StreamSourceKindV1::Nested,
        );
        let payload = StreamItemsPayloadV1::Values(vec![vec![40]]);
        let fresh = live
            .write_attached_items_with_nested(
                &identity.invocation,
                handle.stream_id,
                2,
                payload.clone(),
                vec![nested.clone()],
            )
            .await
            .unwrap();
        assert!(!fresh.replayed);
        assert_eq!(
            live.attached_global_sequence(&identity.invocation, handle.stream_id, 2)
                .await
                .unwrap(),
            5
        );
        let child = live
            .nested_handles(handle.stream_id, 5)
            .await
            .unwrap()
            .pop()
            .unwrap();
        live.append_external_input(
            &identity.invocation,
            child.stream_id,
            None,
            true,
            Some(ExternalProducerV1 {
                id: ExternalProducerIdV1::Attached,
                epoch: 0,
                sequence: 0,
            }),
        )
        .await
        .unwrap();
        drop(live);
        let recovered = producer(oplog, &identity, None).await;
        let retry = recovered
            .write_attached_items_with_nested(
                &identity.invocation,
                handle.stream_id,
                2,
                payload,
                vec![nested],
            )
            .await
            .unwrap();
        assert!(retry.replayed);
        assert_eq!(retry.value, fresh.value);
        assert_eq!(
            recovered.nested_handles(handle.stream_id, 5).await.unwrap(),
            vec![child]
        );
        let high_water = recovered
            .attached_input_high_water(&identity.invocation, handle.stream_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(high_water.highest_contiguous_sequence, 2);
        assert!(!high_water.terminal);
        assert!(matches!(
            recovered
                .append_external_input(
                    &identity.invocation,
                    handle.stream_id,
                    None,
                    true,
                    Some(ExternalProducerV1 {
                        id: ExternalProducerIdV1::Attached,
                        epoch: 0,
                        sequence: 2
                    }),
                )
                .await,
            Err(DurableStreamProducerError::EventConflict)
        ));
        recovered
            .append_external_input(&identity.invocation, handle.stream_id, None, true, None)
            .await
            .unwrap();
        let high_water = recovered
            .attached_input_high_water(&identity.invocation, handle.stream_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(high_water.highest_contiguous_sequence, 3);
        assert!(high_water.terminal);
        let events = recovered.read_segment(&handle, None, None).await.unwrap();
        assert_eq!(
            events
                .iter()
                .map(|event| event.producer_sequence)
                .collect::<Vec<_>>(),
            (0..7).collect::<Vec<_>>()
        );
        assert!(
            events
                .windows(2)
                .all(|pair| pair[0].offset < pair[1].offset)
        );
    }

    #[test]
    async fn external_append_producer_retries_epochs_close_and_recovery() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live = producer(oplog.clone(), &identity, None).await;
        let handle = live
            .register(registration(
                &identity,
                StreamRegistrationCoordinateV1::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodInput,
                    recursive_value_path: vec![StreamValuePathStepV1::TupleElement(99)],
                },
                StreamSourceKindV1::ExternalInlineInput,
            ))
            .await
            .unwrap()
            .value;
        let p0 = ExternalProducerV1 {
            id: ExternalProducerIdV1::Client("p1".into()),
            epoch: 1,
            sequence: 0,
        };
        let accepted = live
            .append_external_input(
                &identity.invocation,
                handle.stream_id,
                Some(StreamItemsPayloadV1::PackedU8(vec![1, 2])),
                false,
                Some(p0.clone()),
            )
            .await
            .unwrap();
        let ExternalAppendOutcomeV1::Accepted(original) = accepted else {
            panic!()
        };
        let (a, b) = tokio::join!(
            live.append_external_input(
                &identity.invocation,
                handle.stream_id,
                Some(StreamItemsPayloadV1::PackedU8(vec![1, 2])),
                false,
                Some(p0.clone())
            ),
            live.append_external_input(
                &identity.invocation,
                handle.stream_id,
                Some(StreamItemsPayloadV1::PackedU8(vec![1, 2])),
                false,
                Some(p0)
            ),
        );
        let duplicate = ExternalAppendOutcomeV1::Duplicate {
            offset: original,
            highest_sequence: Some(0),
        };
        assert_eq!(a.unwrap(), duplicate);
        assert_eq!(b.unwrap(), duplicate);
        let newer = live
            .append_external_input(
                &identity.invocation,
                handle.stream_id,
                Some(StreamItemsPayloadV1::PackedU8(vec![3])),
                false,
                Some(ExternalProducerV1 {
                    id: ExternalProducerIdV1::Client("p1".into()),
                    epoch: 1,
                    sequence: 1,
                }),
            )
            .await
            .unwrap();
        assert!(matches!(newer, ExternalAppendOutcomeV1::Accepted(_)));
        assert_eq!(
            live.append_external_input(
                &identity.invocation,
                handle.stream_id,
                Some(StreamItemsPayloadV1::PackedU8(vec![1, 2])),
                false,
                Some(ExternalProducerV1 {
                    id: ExternalProducerIdV1::Client("p1".into()),
                    epoch: 1,
                    sequence: 0,
                }),
            )
            .await
            .unwrap(),
            ExternalAppendOutcomeV1::Duplicate {
                offset: original,
                highest_sequence: Some(1),
            }
        );
        assert_eq!(
            live.append_external_input(
                &identity.invocation,
                handle.stream_id,
                Some(StreamItemsPayloadV1::PackedU8(vec![3])),
                false,
                Some(ExternalProducerV1 {
                    id: ExternalProducerIdV1::Client("p1".into()),
                    epoch: 1,
                    sequence: 3
                })
            )
            .await
            .unwrap(),
            ExternalAppendOutcomeV1::SeqGap {
                expected: 2,
                received: 3
            }
        );
        assert!(matches!(
            live.append_external_input(
                &identity.invocation,
                handle.stream_id,
                Some(StreamItemsPayloadV1::PackedU8(vec![3])),
                false,
                Some(ExternalProducerV1 {
                    id: ExternalProducerIdV1::Client("p1".into()),
                    epoch: 2,
                    sequence: 0
                })
            )
            .await
            .unwrap(),
            ExternalAppendOutcomeV1::Accepted(_)
        ));
        assert_eq!(
            live.append_external_input(
                &identity.invocation,
                handle.stream_id,
                Some(StreamItemsPayloadV1::PackedU8(vec![4])),
                false,
                Some(ExternalProducerV1 {
                    id: ExternalProducerIdV1::Client("p1".into()),
                    epoch: 1,
                    sequence: 1
                })
            )
            .await
            .unwrap(),
            ExternalAppendOutcomeV1::EpochFenced(2)
        );
        let normal = live
            .write_items(handle.stream_id, 4, StreamItemsPayloadV1::PackedU8(vec![8]))
            .await
            .unwrap()
            .value[0];
        let closed = live
            .append_external_input(
                &identity.invocation,
                handle.stream_id,
                Some(StreamItemsPayloadV1::PackedU8(vec![4])),
                true,
                Some(ExternalProducerV1 {
                    id: ExternalProducerIdV1::Client("p2".into()),
                    epoch: 0,
                    sequence: 0,
                }),
            )
            .await
            .unwrap();
        let ExternalAppendOutcomeV1::Accepted(closed) = closed else {
            panic!()
        };
        assert!(original < normal && normal < closed);
        assert_eq!(
            live.append_external_input(&identity.invocation, handle.stream_id, None, true, None)
                .await
                .unwrap(),
            ExternalAppendOutcomeV1::Duplicate {
                offset: closed,
                highest_sequence: None,
            }
        );
        drop(live);
        let recovered = producer(oplog, &identity, None).await;
        assert_eq!(
            recovered
                .append_external_input(
                    &identity.invocation,
                    handle.stream_id,
                    Some(StreamItemsPayloadV1::PackedU8(vec![9])),
                    false,
                    None
                )
                .await
                .unwrap(),
            ExternalAppendOutcomeV1::Closed
        );
    }

    #[test]
    async fn close_from_a_different_producer_is_not_reported_as_duplicate() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live = producer(oplog.clone(), &identity, None).await;
        let handle = live
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
        let producer_tuple = |id: &str, sequence| ExternalProducerV1 {
            id: ExternalProducerIdV1::Client(id.into()),
            epoch: 0,
            sequence,
        };
        live.append_external_input(
            &identity.invocation,
            handle.stream_id,
            Some(StreamItemsPayloadV1::PackedU8(vec![1])),
            false,
            Some(producer_tuple("first", 0)),
        )
        .await
        .unwrap();
        let accepted = live
            .append_external_input(
                &identity.invocation,
                handle.stream_id,
                None,
                true,
                Some(producer_tuple("first", 1)),
            )
            .await
            .unwrap();
        let ExternalAppendOutcomeV1::Accepted(offset) = accepted else {
            panic!("first producer close must be accepted");
        };
        let before = oplog.current_oplog_index().await;
        let recovered = producer(oplog.clone(), &identity, None).await;

        for current in [&live, &recovered] {
            assert_eq!(
                current
                    .append_external_input(
                        &identity.invocation,
                        handle.stream_id,
                        None,
                        true,
                        Some(producer_tuple("first", 1)),
                    )
                    .await
                    .unwrap(),
                ExternalAppendOutcomeV1::Duplicate {
                    offset,
                    highest_sequence: Some(1),
                }
            );
            for request in [
                producer_tuple("different", 0),
                producer_tuple("first", 2),
                ExternalProducerV1 {
                    epoch: 1,
                    ..producer_tuple("first", 1)
                },
            ] {
                assert_eq!(
                    current
                        .append_external_input(
                            &identity.invocation,
                            handle.stream_id,
                            None,
                            true,
                            Some(request),
                        )
                        .await
                        .unwrap(),
                    ExternalAppendOutcomeV1::Closed
                );
            }
            assert_eq!(
                current
                    .append_external_input(
                        &identity.invocation,
                        handle.stream_id,
                        Some(StreamItemsPayloadV1::PackedU8(vec![1])),
                        false,
                        Some(producer_tuple("first", 0)),
                    )
                    .await
                    .unwrap(),
                ExternalAppendOutcomeV1::Closed
            );
            assert_eq!(oplog.current_oplog_index().await, before);
        }
    }

    #[test]
    async fn producer_frames_after_earlier_input_consumer_cancel_are_fenced() {
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
                .write_items(
                    handle.stream_id,
                    1,
                    StreamItemsPayloadV1::Values(vec![vec![2]]),
                )
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

        producer
            .register_result_streams(identity.invocation.clone(), vec![1], Vec::new(), None)
            .await
            .unwrap();
        let committed = oplog.committed_length();

        producer
            .register_result_streams(identity.invocation.clone(), vec![1], Vec::new(), None)
            .await
            .unwrap();
        assert_eq!(oplog.committed_length(), committed);

        assert_eq!(
            producer
                .register_result_streams(identity.invocation.clone(), vec![2], Vec::new(), None)
                .await
                .unwrap_err(),
            DurableStreamProducerError::RegistrationDivergence
        );
        assert_eq!(oplog.committed_length(), committed);
    }

    #[test]
    async fn result_plan_preserves_mixed_output_order_and_replays() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog.clone(), &identity, None).await;
        let existing = producer
            .register(root_registration(&identity))
            .await
            .unwrap()
            .value;
        let mut request = root_registration(&identity);
        let StreamRegistrationCoordinateV1::Root {
            recursive_value_path,
            ..
        } = &mut request.coordinate
        else {
            unreachable!();
        };
        recursive_value_path.push(StreamValuePathStepV1::RecordField(1));
        let outputs = || {
            vec![
                ProducerOutputRegistrationV1 {
                    transport_stream_id: 31,
                    source: ProducerOutputSourceV1::New(request.clone()),
                    cancellation_epoch: None,
                },
                ProducerOutputRegistrationV1 {
                    transport_stream_id: 12,
                    source: ProducerOutputSourceV1::Existing(existing.clone()),
                    cancellation_epoch: None,
                },
            ]
        };
        let (owned, record) = producer
            .register_result_streams(identity.invocation.clone(), vec![4, 5], outputs(), None)
            .await
            .unwrap();
        assert_eq!(owned.len(), 1);
        let StreamSessionRecordV1::InvocationResult(result) = &record else {
            unreachable!();
        };
        assert_eq!(result.session_key, identity.invocation);
        assert_eq!(result.result, vec![4, 5]);
        assert_eq!(
            result.output_streams,
            vec![owned[0].clone(), existing.clone()]
        );
        assert_eq!(
            result
                .stream_mappings
                .iter()
                .map(|mapping| mapping.transport_stream_id)
                .collect::<Vec<_>>(),
            vec![31, 12]
        );
        let committed = oplog.committed_length();
        let replay = producer
            .register_result_streams(identity.invocation.clone(), vec![4, 5], outputs(), None)
            .await
            .unwrap();
        assert_eq!(replay, (owned, record));
        assert_eq!(oplog.committed_length(), committed);
    }

    #[test]
    async fn result_registration_cancels_outputs_before_publishing_the_result() {
        for new_output in [false, true] {
            let identity = identity();
            let oplog = Arc::new(TestOplog::default());
            let producer = producer(oplog.clone(), &identity, None).await;
            let mut request = root_registration(&identity);
            request.coordinate = StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: vec![StreamValuePathStepV1::RecordField(0)],
            };
            let existing = producer.register(request.clone()).await.unwrap().value;
            producer
                .end(existing.stream_id, 0, StreamEndResultV1::Ok)
                .await
                .unwrap();
            request.coordinate = StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: vec![StreamValuePathStepV1::RecordField(1)],
            };
            let open = if new_output {
                None
            } else {
                Some(producer.register(request.clone()).await.unwrap().value)
            };
            let outputs = || {
                vec![
                    ProducerOutputRegistrationV1 {
                        transport_stream_id: 13,
                        source: match &open {
                            Some(handle) => ProducerOutputSourceV1::Existing(handle.clone()),
                            None => ProducerOutputSourceV1::New(request.clone()),
                        },
                        cancellation_epoch: Some(7),
                    },
                    ProducerOutputRegistrationV1 {
                        transport_stream_id: 29,
                        source: ProducerOutputSourceV1::Existing(existing.clone()),
                        cancellation_epoch: Some(7),
                    },
                ]
            };
            let registered = producer
                .register_result_streams(identity.invocation.clone(), vec![3], outputs(), None)
                .await
                .unwrap();
            let StreamSessionRecordV1::InvocationResult(result) = &registered.1 else {
                unreachable!()
            };
            let result_index = oplog.current_oplog_index().await;
            let recovered = DurableStreamProducer::load(
                oplog.clone(),
                identity.environment_id,
                identity.agent_id.clone(),
                identity.fingerprint,
                None,
            )
            .await
            .unwrap();
            for (position, handle) in result.output_streams.iter().enumerate() {
                let mut reader = recovered.catch_up(handle.clone(), None).await.unwrap();
                let event = reader.next().await.unwrap().unwrap();
                assert!(event.offset.producer_oplog_index() < result_index);
                assert_eq!(
                    event.payload,
                    if position == 0 {
                        CommittedProducerStreamEventPayloadV1::Cancel {
                            role: StreamCancelRoleV1::OutputConsumer,
                            reason: StreamCancelReasonV1::Cancelled,
                            details: None,
                        }
                    } else {
                        CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::Ok)
                    }
                );
                assert!(reader.next().await.unwrap().is_none());
            }
            assert_eq!(
                recovered
                    .register_result_streams(identity.invocation.clone(), vec![3], outputs(), None)
                    .await
                    .unwrap(),
                registered
            );
            assert_eq!(oplog.current_oplog_index().await, result_index);
        }
    }

    #[test]
    async fn result_plan_rejects_duplicate_new_coordinates_before_committing() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog.clone(), &identity, None).await;
        let request = root_registration(&identity);
        let outputs = vec![
            ProducerOutputRegistrationV1 {
                transport_stream_id: 1,
                source: ProducerOutputSourceV1::New(request.clone()),
                cancellation_epoch: None,
            },
            ProducerOutputRegistrationV1 {
                transport_stream_id: 2,
                source: ProducerOutputSourceV1::New(request),
                cancellation_epoch: None,
            },
        ];

        assert_eq!(
            producer
                .register_result_streams(identity.invocation.clone(), vec![1], outputs, None)
                .await,
            Err(DurableStreamProducerError::RegistrationDivergence)
        );
        assert_eq!(oplog.committed_length(), 0);
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
                vec![DurableStreamOplogRecord::Items(
                    None,
                    StreamItemsRecordV1 {
                        format_version: 1,
                        stream_id,
                        producer_fingerprint,
                        first_sequence: 1,
                        nested_stream_ids: Vec::new(),
                        newly_registered_stream_ids: Vec::new(),
                        payload: StreamItemsPayloadV1::Values(vec![vec![1]]),
                        offsets: vec![StreamOffsetV1::new(item_index, 0)],
                    },
                )]
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
                None,
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
                None,
                vec![(nested_index, None, nested)],
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
                    DurableStreamOplogRecord::Registered(None, nested_record),
                    DurableStreamOplogRecord::Items(
                        None,
                        StreamItemsRecordV1 {
                            format_version: DURABLE_STREAM_FORMAT_VERSION,
                            stream_id: parent_stream_id,
                            producer_fingerprint,
                            first_sequence: 0,
                            nested_stream_ids: vec![nested_stream_id, nested_stream_id],
                            newly_registered_stream_ids: vec![nested_stream_id],
                            payload: StreamItemsPayloadV1::Values(vec![vec![1]]),
                            offsets: vec![StreamOffsetV1::new(item_index, 0)],
                        },
                    ),
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
                vec![DurableStreamOplogRecord::Registered(
                    None,
                    registration_record(
                        registration_index,
                        environment_id,
                        agent_id,
                        producer_fingerprint,
                        nested,
                    ),
                )]
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
                entity_parent_start_index: None,
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
                .apply_session_references(None, &record(mapping(position)))
                .unwrap();
        }
        assert_eq!(
            index.session_stream_counts[&identity.invocation],
            MAX_DURABLE_STREAMS_PER_SESSION
        );

        index
            .apply_session_references(None, &record(mapping(0)))
            .unwrap();
        assert_eq!(
            index.session_stream_counts[&identity.invocation],
            MAX_DURABLE_STREAMS_PER_SESSION
        );
        assert_eq!(
            index.apply_session_references(None, &record(mapping(MAX_DURABLE_STREAMS_PER_SESSION))),
            Err(DurableStreamProducerError::StreamLimit)
        );
    }

    #[test]
    async fn session_control_batch_validates_before_appending_any_record() {
        use crate::services::oplog::OplogOps;
        use golem_common::model::durable_stream::{
            StreamSessionCancelRequestedRecordV1, StreamSlotTombstonedRecordV1,
        };

        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = producer(oplog.clone(), &identity, None).await;
        let cancel = StreamSessionRecordV1::CancelRequested(StreamSessionCancelRequestedRecordV1 {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            session_key: identity.invocation.clone(),
        });
        let tombstone = StreamSessionRecordV1::Tombstoned(StreamSlotTombstonedRecordV1 {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            session_key: identity.invocation,
            slot: "$result".into(),
            role: SessionStreamRoleV1::Output,
        });
        let mut malformed = tombstone.clone();
        if let StreamSessionRecordV1::Tombstoned(record) = &mut malformed {
            record.slot.clear();
        }
        let before = oplog.current_oplog_index().await;
        let invalid_records = vec![cancel.clone(), malformed];
        assert!(matches!(
            producer
                .run_owned(0, move |owner| async move {
                    owner
                        .append_session_records_owned(None, invalid_records)
                        .await
                })
                .await,
            Err(DurableStreamProducerError::CorruptHistory(_))
        ));
        assert_eq!(oplog.current_oplog_index().await, before);

        let records = vec![cancel.clone(), tombstone.clone()];
        producer
            .run_owned(0, move |owner| async move {
                owner.append_session_records_owned(None, records).await
            })
            .await
            .unwrap();
        assert_eq!(
            oplog.current_oplog_index().await.as_u64(),
            before.as_u64() + 2
        );
        for (offset, expected) in [cancel, tombstone].into_iter().enumerate() {
            let entry = oplog
                .read(OplogIndex::from_u64(before.as_u64() + 1 + offset as u64))
                .await;
            let OplogEntry::StreamSession { record, .. } = entry else {
                panic!("expected session control record");
            };
            assert_eq!(oplog.download_payload(record).await.unwrap(), expected);
        }
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
    async fn remote_cancellation_releases_durable_activity_but_retains_owned_admission() {
        for abandon in [false, true] {
            let live = producer(Arc::new(TestOplog::default()), &identity(), None).await;
            let (started, ready) = oneshot::channel();
            let (release, released) = oneshot::channel();
            let caller = tokio::spawn({
                let live = live.clone();
                async move {
                    live.run_lifecycle(7, move |owner| async move {
                        let routed_owner = owner.clone();
                        owner.defer_remote_cancellation(async move {
                            assert!(super::MUTATION_SCOPE.try_with(|_| ()).is_err());
                            started.send(()).unwrap();
                            released.await.unwrap();
                            assert_eq!(
                                routed_owner
                                    .run_owned(0, |_| async {
                                        Ok::<(), DurableStreamProducerError>(())
                                    })
                                    .await,
                                Err(DurableStreamProducerError::RecoveryRequired)
                            );
                            Err(DurableStreamProducerError::Oplog("remote failure".into()))
                        });
                        Ok::<(), DurableStreamProducerError>(())
                    })
                    .await
                }
            });
            ready.await.unwrap();
            assert!(!caller.is_finished());
            live.durable_activity.close();
            tokio::time::timeout(Duration::from_secs(5), live.wait_durable_drained())
                .await
                .expect("remote RPC retained local durable activity");
            assert_eq!(live.lifecycle_operations.available_permits(), 15);
            assert_eq!(
                live.lifecycle_operation_bytes.available_permits(),
                256 * 1024 * 1024 - 7
            );
            if abandon {
                caller.abort();
            }
            release.send(()).unwrap();
            if abandon {
                assert!(caller.await.unwrap_err().is_cancelled());
            } else {
                assert_eq!(
                    caller.await.unwrap(),
                    Err(DurableStreamProducerError::Oplog("remote failure".into()))
                );
            }
            tokio::time::timeout(Duration::from_secs(5), async {
                while live.lifecycle_operations.available_permits() != 16 {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            assert_eq!(
                live.lifecycle_operation_bytes.available_permits(),
                256 * 1024 * 1024
            );
            live.ensure_healthy().unwrap();
        }
    }

    #[test]
    #[timeout("30s")]
    async fn session_notification_waits_for_status_fold_after_caller_cancellation() {
        let identity = identity();
        let release = Arc::new(Notify::new());
        let folded = Arc::new(AtomicBool::new(false));
        let commit: DurableStreamCommit = Arc::new({
            let release = release.clone();
            let folded = folded.clone();
            move |receipt| {
                let release = release.clone();
                let folded = folded.clone();
                Box::pin(async move {
                    receipt.unwrap().send(()).unwrap();
                    release.notified().await;
                    folded.store(true, Ordering::Release);
                })
            }
        });
        let live = DurableStreamProducer::load_with_commit(
            Arc::new(TestOplog::default()),
            identity.environment_id,
            identity.agent_id,
            identity.fingerprint,
            None,
            commit,
        )
        .await
        .unwrap();
        let mut notification = Box::pin(live.session_records_changed().notified());
        notification.as_mut().enable();
        let (requested, ready) = oneshot::channel();
        let caller = tokio::spawn({
            let live = live.clone();
            async move {
                live.run_owned(0, move |owner| async move {
                    owner.commit().await;
                    owner.finish_durable_effect();
                    owner.notify_session_records_changed();
                    requested.send(()).unwrap();
                    Ok::<(), DurableStreamProducerError>(())
                })
                .await
            }
        });
        ready.await.unwrap();
        assert!(futures::poll!(notification.as_mut()).is_pending());
        assert!(!folded.load(Ordering::Acquire));
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        release.notify_one();
        notification.await;
        assert!(folded.load(Ordering::Acquire));
        live.wait_durable_drained().await;
    }

    #[test]
    #[timeout("30s")]
    async fn durable_activity_waits_for_callback_tails_but_not_abandoned_fanout() {
        for lifecycle in [false, true] {
            let identity = identity();
            let oplog = Arc::new(TestOplog::default());
            let block = Arc::new(AtomicBool::new(false));
            let committed = Arc::new(Notify::new());
            let release = Arc::new(Notify::new());
            let commit: DurableStreamCommit = Arc::new({
                let oplog = oplog.clone();
                let block = block.clone();
                let committed = committed.clone();
                let release = release.clone();
                move |receipt| {
                    let oplog = oplog.clone();
                    let block = block.clone();
                    let committed = committed.clone();
                    let release = release.clone();
                    Box::pin(async move {
                        oplog.commit(CommitLevel::Always).await;
                        if let Some(receipt) = receipt {
                            let _ = receipt.send(());
                        }
                        if block.swap(false, Ordering::AcqRel) {
                            committed.notify_one();
                            release.notified().await;
                        }
                    })
                }
            });
            let live = DurableStreamProducer::load_with_commit(
                oplog,
                identity.environment_id,
                identity.agent_id.clone(),
                identity.fingerprint,
                Some(1),
                commit,
            )
            .await
            .unwrap();
            let handle = live
                .register(root_registration(&identity))
                .await
                .unwrap()
                .value;
            let mut reader = live
                .bus(handle.stream_id)
                .unwrap()
                .subscribe()
                .await
                .unwrap();
            live.write_items(
                handle.stream_id,
                0,
                StreamItemsPayloadV1::Values(vec![vec![3]]),
            )
            .await
            .unwrap();
            block.store(true, Ordering::Release);
            let caller = tokio::spawn({
                let live = live.clone();
                async move {
                    if lifecycle {
                        live.end_open(handle.stream_id, StreamEndResultV1::Ok).await
                    } else {
                        live.write_items(
                            handle.stream_id,
                            1,
                            StreamItemsPayloadV1::Values(vec![vec![7]]),
                        )
                        .await
                        .map(|_| ())
                    }
                }
            });
            committed.notified().await;
            caller.abort();
            assert!(caller.await.unwrap_err().is_cancelled());
            live.durable_activity.close();
            let drained = live.durable_activity.wait_drained();
            tokio::pin!(drained);
            assert!(futures::poll!(&mut drained).is_pending());
            release.notify_one();
            tokio::time::timeout(Duration::from_secs(5), &mut drained)
                .await
                .expect("durable activity retained a blocked publication");
            assert_eq!(
                live.run_owned(0, |_| async { Ok::<(), DurableStreamProducerError>(()) })
                    .await,
                Err(DurableStreamProducerError::RecoveryRequired)
            );
            if !lifecycle {
                assert_eq!(live.owned_operations.available_permits(), 15);
            }
            let first = reader.recv().await.unwrap();
            let second = reader.recv().await.unwrap();
            assert_eq!(
                first.payload.payload,
                CommittedProducerStreamEventPayloadV1::Value(vec![3])
            );
            assert!(first.offset < second.offset);
            assert_eq!(
                second.payload.payload,
                if lifecycle {
                    CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::Ok)
                } else {
                    CommittedProducerStreamEventPayloadV1::Value(vec![7])
                }
            );
        }
    }

    #[test]
    #[timeout("30s")]
    async fn lifecycle_cancellations_outlive_callers_under_saturated_data_admission() {
        for saturate_bytes in [false, true] {
            let identity = identity();
            let oplog = Arc::new(TestOplog::default());
            let live = producer(oplog.clone(), &identity, Some(1)).await;
            let mut streams = Vec::new();
            // More blocked terminals than the lifecycle lane can admit at once, and more
            // buses than one dispatcher page, exercise both admission and scan progress.
            for i in 0..33 {
                let handle = live
                    .register(registration(
                        &identity,
                        StreamRegistrationCoordinateV1::Root {
                            invocation_id: identity.invocation.clone(),
                            root_kind: StreamRootKindV1::MethodResult,
                            recursive_value_path: vec![StreamValuePathStepV1::TupleElement(i)],
                        },
                        StreamSourceKindV1::InvocationOutput,
                    ))
                    .await
                    .unwrap()
                    .value;
                let reader = live
                    .bus(handle.stream_id)
                    .unwrap()
                    .subscribe()
                    .await
                    .unwrap();
                live.write_items(
                    handle.stream_id,
                    0,
                    StreamItemsPayloadV1::Values(vec![vec![3]]),
                )
                .await
                .unwrap();
                streams.push((handle.stream_id, reader));
            }
            let blocked_count = if saturate_bytes { 1 } else { 16 };
            let mut writes = Vec::new();
            for (stream_id, _) in streams.iter().take(blocked_count) {
                let stream_id = *stream_id;
                let live = live.clone();
                writes.push(tokio::spawn(async move {
                    // Charge the whole byte lane without allocating a 256 MiB test payload.
                    // The nested write retains that reservation through blocked delivery.
                    live.run_owned(
                        if saturate_bytes { 256 * 1024 * 1024 } else { 1 },
                        move |owner| async move {
                            owner
                                .write_items(
                                    stream_id,
                                    1,
                                    StreamItemsPayloadV1::Values(vec![vec![7]]),
                                )
                                .await
                        },
                    )
                    .await
                }));
            }
            while oplog.committed_length() < (66 + blocked_count) as u64 {
                tokio::task::yield_now().await;
            }
            if saturate_bytes {
                assert_eq!(live.owned_operation_bytes.available_permits(), 0);
            } else {
                assert_eq!(live.owned_operations.available_permits(), 0);
            }
            let mut cancellations = Vec::new();
            for (stream_id, _) in &streams {
                let stream_id = *stream_id;
                let live = live.clone();
                cancellations.push(tokio::spawn(async move {
                    live.cancel_open(
                        stream_id,
                        StreamCancelRoleV1::OutputConsumer,
                        StreamCancelReasonV1::Cancelled,
                        None,
                    )
                    .await
                }));
            }
            loop {
                let index = live.index.lock().await;
                let committed = index
                    .streams
                    .values()
                    .filter(|stream| stream.terminal)
                    .count();
                drop(index);
                if committed == 33 && live.lifecycle_operations.available_permits() == 16 {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert!(writes.iter().all(|write| !write.is_finished()));
            assert!(cancellations.iter().all(|cancel| !cancel.is_finished()));
            for cancel in cancellations.iter().step_by(2) {
                cancel.abort();
            }
            for (i, (_, mut reader)) in streams.into_iter().enumerate() {
                let first = reader.recv().await.unwrap();
                assert_eq!(
                    first.payload.payload,
                    CommittedProducerStreamEventPayloadV1::Value(vec![3])
                );
                let mut previous = first.offset;
                if i < blocked_count {
                    let second = reader.recv().await.unwrap();
                    assert_eq!(
                        second.payload.payload,
                        CommittedProducerStreamEventPayloadV1::Value(vec![7])
                    );
                    assert!(previous < second.offset);
                    previous = second.offset;
                }
                let terminal = reader.recv().await.unwrap();
                assert!(previous < terminal.offset);
                assert_eq!(
                    terminal.payload.payload,
                    CommittedProducerStreamEventPayloadV1::Cancel {
                        role: StreamCancelRoleV1::OutputConsumer,
                        reason: StreamCancelReasonV1::Cancelled,
                        details: None,
                    }
                );
            }
            for write in writes {
                write.await.unwrap().unwrap();
            }
            for (i, cancel) in cancellations.into_iter().enumerate() {
                if i % 2 == 0 {
                    assert!(cancel.await.unwrap_err().is_cancelled());
                } else {
                    cancel.await.unwrap().unwrap();
                }
            }
            assert_eq!(live.owned_operations.available_permits(), 16);
            assert_eq!(
                live.owned_operation_bytes.available_permits(),
                256 * 1024 * 1024
            );
        }
    }

    #[test]
    #[timeout("30s")]
    async fn session_finish_reserves_repeated_terminal_errors_before_appending() {
        use crate::services::oplog::OplogOps;
        use golem_common::base_model::durable_stream::StreamSessionFinishedRecordV1;

        for error_len in [128 * 1024, 2 * 1024 * 1024] {
            let identity = identity();
            let oplog = Arc::new(TestOplog::default());
            let live = producer(oplog.clone(), &identity, None).await;
            let mut handles = Vec::new();
            for i in 0..3 {
                handles.push(
                    live.register(registration(
                        &identity,
                        StreamRegistrationCoordinateV1::Root {
                            invocation_id: identity.invocation.clone(),
                            root_kind: StreamRootKindV1::MethodResult,
                            recursive_value_path: vec![StreamValuePathStepV1::TupleElement(i)],
                        },
                        StreamSourceKindV1::InvocationOutput,
                    ))
                    .await
                    .unwrap()
                    .value,
                );
            }
            let before = oplog.current_oplog_index().await;
            let occupied = live
                .lifecycle_operation_bytes
                .clone()
                .acquire_many_owned(256 * 1024 * 1024 - 128)
                .await
                .unwrap();
            let error = vec![71; error_len];
            let mut finishing = Box::pin(live.finish_session(
                identity.invocation.clone(),
                None,
                Err(error.clone()),
                StreamCancelReasonV1::InvocationFailed,
            ));
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(50), &mut finishing)
                    .await
                    .is_err()
            );
            assert_eq!(oplog.current_oplog_index().await, before);
            drop(occupied);
            finishing.await.unwrap();
            let finished_index = oplog.current_oplog_index().await;
            assert_eq!(finished_index.as_u64(), before.as_u64() + 4);
            let OplogEntry::StreamSession { record, .. } = oplog.read(finished_index).await else {
                panic!("terminal batch must end with Finished");
            };
            assert_eq!(
                oplog.download_payload(record).await.unwrap(),
                StreamSessionRecordV1::Finished(StreamSessionFinishedRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: identity.invocation.clone(),
                    result: Err(error.clone()),
                })
            );
            assert!(
                live.index
                    .lock()
                    .await
                    .streams
                    .values()
                    .all(|stream| stream.terminal_event.is_none())
            );
            let restarted = producer(oplog.clone(), &identity, None).await;
            for current in [&live, &restarted] {
                for handle in &handles {
                    let mut reader = current.catch_up(handle.clone(), None).await.unwrap();
                    assert_eq!(
                        reader.next().await.unwrap().unwrap().payload,
                        CommittedProducerStreamEventPayloadV1::End(
                            StreamEndResultV1::ErrorContext(error.clone())
                        )
                    );
                }
                let occupied = current
                    .lifecycle_operation_bytes
                    .clone()
                    .acquire_many_owned(256 * 1024 * 1024)
                    .await
                    .unwrap();
                tokio::time::timeout(
                    Duration::from_secs(1),
                    current.finish_session(
                        identity.invocation.clone(),
                        None,
                        Err(vec![99; error_len]),
                        StreamCancelReasonV1::InvocationFailed,
                    ),
                )
                .await
                .unwrap()
                .unwrap();
                assert_eq!(oplog.current_oplog_index().await, finished_index);
                drop(occupied);
            }
            assert_eq!(
                live.lifecycle_operation_bytes.available_permits(),
                256 * 1024 * 1024
            );
        }
    }

    #[test]
    #[timeout("30s")]
    async fn session_finish_memory_does_not_scale_with_stream_count() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let live = producer(oplog.clone(), &identity, None).await;
        for i in 0..MAX_DURABLE_STREAMS_PER_SESSION {
            live.register(registration(
                &identity,
                StreamRegistrationCoordinateV1::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodResult,
                    recursive_value_path: vec![StreamValuePathStepV1::TupleElement(i as u32)],
                },
                StreamSourceKindV1::InvocationOutput,
            ))
            .await
            .unwrap();
        }
        let before = oplog.current_oplog_index().await;
        let occupied = live
            .lifecycle_operation_bytes
            .clone()
            .acquire_many_owned(256 * 1024 * 1024 - 1024)
            .await
            .unwrap();
        tokio::time::timeout(
            Duration::from_secs(5),
            live.finish_session(
                identity.invocation.clone(),
                None,
                Err(vec![83; 128]),
                StreamCancelReasonV1::InvocationFailed,
            ),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            oplog.current_oplog_index().await.as_u64(),
            before.as_u64() + MAX_DURABLE_STREAMS_PER_SESSION as u64 + 1
        );
        assert!(
            live.index
                .lock()
                .await
                .streams
                .values()
                .all(|stream| stream.terminal && stream.terminal_event.is_none())
        );
        drop(occupied);

        live.run_lifecycle(256 * 1024 * 1024 + 1, |owner| async move {
            assert_eq!(owner.lifecycle_operation_bytes.available_permits(), 0);
            owner
                .run_lifecycle(1, |_| async { Ok::<(), DurableStreamProducerError>(()) })
                .await
        })
        .await
        .unwrap();
        assert_eq!(
            live.lifecycle_operation_bytes.available_permits(),
            256 * 1024 * 1024
        );
    }

    #[test]
    #[timeout("30s")]
    async fn multistream_lifecycle_batches_release_admission_before_delivery() {
        for deleting in [false, true] {
            let identity = identity();
            let oplog = Arc::new(TestOplog::default());
            let live = producer(oplog.clone(), &identity, Some(1)).await;
            let mut readers = Vec::new();
            for i in 0..3 {
                let handle = live
                    .register(registration(
                        &identity,
                        StreamRegistrationCoordinateV1::Root {
                            invocation_id: identity.invocation.clone(),
                            root_kind: StreamRootKindV1::MethodResult,
                            recursive_value_path: vec![StreamValuePathStepV1::TupleElement(i)],
                        },
                        StreamSourceKindV1::InvocationOutput,
                    ))
                    .await
                    .unwrap()
                    .value;
                let reader = live
                    .bus(handle.stream_id)
                    .unwrap()
                    .subscribe()
                    .await
                    .unwrap();
                live.write_items(
                    handle.stream_id,
                    0,
                    StreamItemsPayloadV1::Values(vec![vec![i as u8 + 3]]),
                )
                .await
                .unwrap();
                readers.push(reader);
            }
            let finishing = tokio::spawn({
                let live = live.clone();
                let session = identity.invocation.clone();
                async move {
                    if deleting {
                        live.commit_deletion_barrier(123, true).await
                    } else {
                        live.finish_session(
                            session,
                            None,
                            Err(vec![19, 31]),
                            StreamCancelReasonV1::InvocationFailed,
                        )
                        .await
                    }
                }
            });
            loop {
                let index = live.index.lock().await;
                let committed = if deleting {
                    index.deleting
                } else {
                    index.finished_sessions.contains(&identity.invocation)
                };
                drop(index);
                if committed && live.lifecycle_operations.available_permits() == 16 {
                    break;
                }
                tokio::task::yield_now().await;
            }
            if deleting {
                tokio::time::timeout(Duration::from_secs(5), finishing)
                    .await
                    .expect("deletion waited for terminal delivery")
                    .unwrap()
                    .unwrap();
            } else {
                assert!(!finishing.is_finished());
                finishing.abort();
                assert!(finishing.await.unwrap_err().is_cancelled());
            }
            for (i, mut reader) in readers.into_iter().enumerate() {
                let item = reader.recv().await.unwrap();
                assert_eq!(
                    item.payload.payload,
                    CommittedProducerStreamEventPayloadV1::Value(vec![i as u8 + 3])
                );
                let terminal = reader.recv().await.unwrap();
                assert!(item.offset < terminal.offset);
                if deleting {
                    assert!(matches!(
                        terminal.payload.payload,
                        CommittedProducerStreamEventPayloadV1::Cancel {
                            reason: StreamCancelReasonV1::ProducerDeleting,
                            ..
                        }
                    ));
                } else {
                    assert_eq!(
                        terminal.payload.payload,
                        CommittedProducerStreamEventPayloadV1::End(
                            StreamEndResultV1::ErrorContext(vec![19, 31])
                        )
                    );
                }
            }
            let restarted = producer(oplog, &identity, None).await;
            let index = restarted.index.lock().await;
            assert_eq!(
                index
                    .streams
                    .values()
                    .filter(|stream| stream.terminal)
                    .count(),
                3
            );
            assert_eq!(index.deleting, deleting);
            assert_eq!(
                index.finished_sessions.contains(&identity.invocation),
                !deleting
            );
        }
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
            history_source: None,
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
    async fn poisoned_producer_rejects_prefetched_history_without_advancing_cursor() {
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
                StreamItemsPayloadV1::PackedU8(vec![3, 7]),
            )
            .await
            .unwrap();
        let mut reader = producer.catch_up(registered.value, None).await.unwrap();
        assert!(!reader.history.is_empty());
        producer.poison();
        assert_eq!(
            reader.next().await,
            Err(DurableStreamProducerError::RecoveryRequired)
        );
        assert_eq!(
            reader.next().await,
            Err(DurableStreamProducerError::RecoveryRequired)
        );
        assert_eq!(reader.last_delivered, None);
        assert!(!reader.terminal_delivered);
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
                entity_parent_start_index: None,
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
                        None,
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
