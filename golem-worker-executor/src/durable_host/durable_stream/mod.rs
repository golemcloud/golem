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

mod attachment;
mod catch_up;
mod external_input;
mod index;
mod items;
pub(crate) mod metadata;
mod mutation;
mod probe;
mod publication;
mod registration;
mod routing;
mod session;
mod terminals;
#[cfg(test)]
pub(crate) mod tests;

pub(crate) use catch_up::DurableCatchUpReader;
pub(crate) use probe::{
    ConsumerAttachmentStatus, DbDirectStreamAttachmentConsumerProbe, StreamAttachmentConsumerProbe,
};
use publication::CommittedEventRetention;
pub(crate) use routing::{RoutedAttachedStreamSegmentSource, RoutedStreamAttachmentControl};

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
    AttachedStreamSegmentRequest, AttachmentId, DEFAULT_LIVE_JOIN_BUFFER_SIZE,
    DURABLE_STREAM_FORMAT_VERSION, DurableStreamHandle, ExternalProducerId, InputStreamHighWater,
    MAX_DURABLE_STREAM_ITEM_SIZE, MAX_DURABLE_STREAMS_PER_SESSION,
    MAX_NEW_STREAM_HANDLES_PER_VALUE, MAX_PACKED_U8_STREAM_ITEM_SIZE,
    MAX_STREAM_VALUE_TRAVERSAL_DEPTH, STREAM_ATTACHMENT_LEASE_TTL_MILLIS, SessionStreamRole,
    StreamAttachmentActivatedRecord, StreamAttachmentControlOperation,
    StreamAttachmentControlRequest, StreamAttachmentFinalizationReason,
    StreamAttachmentFinalizedRecord, StreamAttachmentKey, StreamAttachmentPreparedRecord,
    StreamAttachmentRenewedRecord, StreamCancelReason, StreamCancelRecord, StreamCancelRole,
    StreamCascadeDependentResult, StreamCascadeOutboxRecord, StreamEndRecord, StreamEndResult,
    StreamExternalProducerStateRecord, StreamId, StreamInvocationId, StreamItemsPayload,
    StreamItemsRecord, StreamOffset, StreamProducerDeletingRecord, StreamRegisteredRecord,
    StreamRegistrationCoordinate, StreamSessionAttachedRecord, StreamSessionFinishedRecord,
    StreamSessionInputHighWaterRecord, StreamSessionKey, StreamSessionMapping,
    StreamSessionMappingRecord, StreamSessionPreparedRecord, StreamSessionRecord, StreamSourceKind,
    StreamSourceUnavailableRecord, StreamTerminalAuthor,
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

#[derive(Clone, Debug, Eq, PartialEq)]
/// Stable producer-side facts required to register a stream in an invocation session.
pub(crate) struct ProducerRegistrationRequest {
    pub(crate) coordinate: StreamRegistrationCoordinate,
    pub(crate) source_invocation: StreamInvocationId,
    pub(crate) entity_parent_start_index: Option<OplogIndex>,
    pub(crate) component_revision: ComponentRevision,
    pub(crate) element_schema_fingerprint: SchemaFingerprintV1,
    pub(crate) source_kind: StreamSourceKind,
    pub(crate) session_mapping: Option<StreamSessionMapping>,
}

#[derive(Clone)]
/// Describes whether a nested stream is created here or forwards an existing durable handle.
pub(crate) enum NestedStreamWrite {
    Register(ProducerRegistrationRequest),
    Forward(DurableStreamHandle),
}

/// Selects a newly registered or already durable source for an output mapping.
pub(crate) enum ProducerOutputSource {
    New(ProducerRegistrationRequest),
    Existing(DurableStreamHandle),
}

/// Associates a transport output slot with its durable producer source.
pub(crate) struct ProducerOutputRegistration {
    pub(crate) transport_stream_id: u64,
    pub(crate) source: ProducerOutputSource,
    pub(crate) cancellation_epoch: Option<u64>,
}

/// A cancellation whose oplog record is durable but whose postcommit publication is pending.
pub(crate) struct PendingCommittedCancellation {
    stream_id: StreamId,
    sequence: u64,
    role: StreamCancelRole,
    reason: StreamCancelReason,
    publication: PublicationReceipt,
    event: CommittedProducerStreamEvent,
    outcome: Result<ProducerWriteOutcome<StreamOffset>, DurableStreamProducerError>,
}

#[derive(Clone, Debug, Eq, PartialEq, desert_rust::BinaryCodec)]
/// Guest-visible payload reconstructed from a committed producer stream record.
pub(crate) enum CommittedProducerStreamEventPayload {
    Value(Vec<u8>),
    PackedU8(u8),
    End(StreamEndResult),
    Cancel {
        role: StreamCancelRole,
        reason: StreamCancelReason,
        details: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, desert_rust::BinaryCodec)]
/// A committed producer event, identified by stream sequence and durable oplog offset.
pub(crate) struct CommittedProducerStreamEvent {
    pub(crate) stream_id: StreamId,
    pub(crate) producer_sequence: u64,
    pub(crate) offset: StreamOffset,
    pub(crate) packed_u8_batch_end: Option<StreamOffset>,
    pub(crate) terminal_author: Option<StreamTerminalAuthor>,
    pub(crate) nested_handles: Vec<DurableStreamHandle>,
    pub(crate) payload: CommittedProducerStreamEventPayload,
}

impl CommittedProducerStreamEvent {
    /// Returns whether this event permanently closes the stream.
    pub(crate) fn is_terminal(&self) -> bool {
        matches!(
            self.payload,
            CommittedProducerStreamEventPayload::End(_)
                | CommittedProducerStreamEventPayload::Cancel { .. }
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Reports a mutation result and whether it was recovered from existing durable state.
pub(crate) struct ProducerWriteOutcome<T> {
    pub(crate) value: T,
    pub(crate) replayed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, desert_rust::BinaryCodec)]
/// A bounded durable read plus the cursor and terminal state needed to continue it.
pub(crate) struct StreamHandleReadResult {
    pub(crate) events: Vec<CommittedProducerStreamEvent>,
    pub(crate) next_offset: Option<StreamOffset>,
    pub(crate) head_offset: Option<StreamOffset>,
    pub(crate) closed: bool,
    pub(crate) cancelled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Identity and monotonic write position of an external producer attachment.
pub(crate) struct ExternalProducer {
    pub(crate) id: ExternalProducerId,
    pub(crate) epoch: u64,
    pub(crate) sequence: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Durable admission result for an externally supplied input event.
pub(crate) enum ExternalAppendOutcome {
    Accepted(StreamOffset),
    Duplicate {
        offset: StreamOffset,
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
/// Contract and persistence failures detected by the producer journal.
pub(crate) enum DurableStreamProducerError {
    UnsupportedVersion(u8),
    InvalidHandle,
    RegistrationDivergence,
    UnknownStream(StreamId),
    AlreadyTerminal(StreamId),
    FencedByTerminal(CommittedProducerStreamEventPayload),
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
    SessionFinished(StreamSessionKey),
    AttachmentConflict,
    StaleEpoch { current: u64, actual: u64 },
    InvalidEpoch { current: u64, actual: u64 },
    InvalidAttachmentState,
    LeaseExpired,
    ProducerDeleting,
    ConsumerDeleting,
    ConsumerJournalAdvanced,
    DeletionBlocked(Vec<StreamAttachmentKey>),
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
    /// Formats the dependent attachment identities that currently block deletion.
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
    registrations: HashMap<StreamId, StreamRegisteredRecord>,
    entity_parent_start_indices: HashMap<StreamId, Option<OplogIndex>>,
    referenced_handles: HashMap<StreamId, (DurableStreamHandle, HashSet<StreamSessionKey>)>,
    coordinates: HashMap<StreamRegistrationCoordinate, StreamId>,
    streams: HashMap<StreamId, IndexedProducerStream>,
    stream_sessions: HashMap<StreamId, StreamSessionKey>,
    stream_roles: HashMap<StreamId, SessionStreamRole>,
    session_stream_mappings:
        HashMap<StreamSessionKey, HashSet<(DurableStreamHandle, SessionStreamRole)>>,
    session_stream_counts: HashMap<StreamSessionKey, usize>,
    session_entity_parent_start_indices: HashMap<StreamSessionKey, Option<OplogIndex>>,
    open_session_streams: HashMap<StreamSessionKey, HashSet<StreamId>>,
    invocation_results: HashMap<StreamSessionKey, OplogIndex>,
    finished_sessions: HashSet<StreamSessionKey>,
    attachments: HashMap<(AttachmentId, StreamId, EnvironmentId, AgentId), IndexedStreamAttachment>,
    active_attachments_by_session_stream: HashMap<(StreamSessionKey, StreamId), u64>,
    active_attachment_count: u64,
    attachment_pages: HashMap<u64, Vec<(AttachmentId, StreamId, EnvironmentId, AgentId)>>,
    attachment_positions: HashMap<(AttachmentId, StreamId, EnvironmentId, AgentId), Option<u64>>,
    batch_positions: HashMap<(StreamId, OplogIndex), (u64, u64)>,
    complete_for_deletion: bool,
    cascade_outbox: HashMap<StreamAttachmentKey, StreamCascadeDependentResult>,
    consumer_journals: HashMap<(StreamSessionKey, StreamId), IndexedConsumerJournal>,
    external_producer_heads:
        HashMap<(StreamSessionKey, StreamId, ExternalProducerId), IndexedExternalProducer>,
    external_producer_offsets:
        HashMap<(StreamSessionKey, StreamId, ExternalProducerId, u64, u64), StreamOffset>,
    open_streams: usize,
    deleting: bool,
    consumer_deleting: bool,
}

#[derive(Clone, Default, desert_rust::BinaryCodec)]
/// Reconstructed consumer progress for one session stream.
pub struct IndexedConsumerJournal {
    next_read_ordinal: u64,
    terminal: bool,
    last_source_offset: Option<StreamOffset>,
    source_unavailable: Option<(StreamAttachmentKey, StreamOffset)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProducerJournalSummary {
    event_count: u64,
    last_offset: Option<StreamOffset>,
    terminal: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Folded count, cursor, and terminal state of a consumer journal.
pub(crate) struct ConsumerJournalSummary {
    event_count: u64,
    last_offset: Option<StreamOffset>,
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
        reason: StreamAttachmentFinalizationReason,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, desert_rust::BinaryCodec)]
/// Reconstructed lifecycle state for a producer-to-consumer attachment.
pub struct IndexedStreamAttachment {
    key: StreamAttachmentKey,
    state: IndexedStreamAttachmentState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Externally relevant phase of a durable stream attachment.
pub(crate) enum StreamAttachmentState {
    Prepared,
    Active,
    Finalized(StreamAttachmentFinalizationReason),
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Current attachment phase and, while resumable, its lease deadline.
pub(crate) struct StreamAttachmentView {
    pub(crate) key: StreamAttachmentKey,
    pub(crate) state: StreamAttachmentState,
    pub(crate) lease_expires_at_millis: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Durable evidence used to explain or perform producer deletion.
pub(crate) struct StreamDeletionDiagnostics {
    pub(crate) deleting: bool,
    pub(crate) attachments: Vec<StreamAttachmentView>,
    pub(crate) cascade_completed: Vec<(StreamAttachmentKey, StreamCascadeDependentResult)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AttachmentApplyOutcome {
    Changed,
    Replayed,
}

#[derive(Clone, Default)]
struct IndexedProducerStream {
    terminal_event: Option<CommittedProducerStreamEvent>,
    batches: BTreeMap<u64, OplogIndex>,
    first_sequence: Option<u64>,
    next_sequence: u64,
    last_offset: Option<StreamOffset>,
    terminal: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, desert_rust::BinaryCodec)]
/// Reconstructed epoch and sequence head for one external producer.
pub struct IndexedExternalProducer {
    epoch: u64,
    last_sequence: u64,
    next_sequence: u64,
    last_offset: StreamOffset,
}

/// Commits appended stream records and optionally acknowledges durable completion.
pub(crate) type DurableStreamCommit = Arc<
    dyn Fn(Option<oneshot::Sender<()>>) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync,
>;

/// Producer journal and disposable live publication state for streams owned by one agent.
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
    mutations: mutation::MutationQueue,
    owned_operations: Arc<Semaphore>,
    owned_operation_bytes: Arc<Semaphore>,
    lifecycle_operations: Arc<Semaphore>,
    lifecycle_operation_bytes: Arc<Semaphore>,
    buses: RwLock<BTreeMap<StreamId, Arc<DurableLiveStreamBus<CommittedProducerStreamEvent>>>>,
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
        std::sync::Mutex<HashMap<StreamSessionKey, std::sync::Weak<tokio::sync::Mutex<()>>>>,
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
        DurableLiveStreamBus::<CommittedProducerStreamEvent>::new(live_join_capacity)?;
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
                            StreamRegistrationCoordinate::Nested { .. }
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
                        if let StreamSessionRecord::ExternalProducerState(value) = &record {
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
                        if let StreamSessionRecord::Finished(record) = &record {
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
            mutations: mutation::MutationQueue::new(),
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

    /// Returns the environment containing the producer journal.
    pub(crate) fn environment_id(&self) -> EnvironmentId {
        self.environment_id
    }

    /// Returns the agent whose oplog owns these streams.
    pub(crate) fn agent_id(&self) -> &AgentId {
        &self.producer
    }

    /// Returns the durable producer identity that fences recreated agents.
    pub(crate) fn fingerprint(&self) -> AgentFingerprint {
        self.producer_fingerprint
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

#[async_trait]
/// Reads committed producer history without relying on a resident live subscription.
pub(crate) trait StreamSegmentSource: Send + Sync {
    /// Reads committed events after `after`, optionally stopping at `through`.
    async fn read_segment(
        &self,
        handle: &DurableStreamHandle,
        after: Option<StreamOffset>,
        through: Option<StreamOffset>,
    ) -> Result<Vec<CommittedProducerStreamEvent>, DurableStreamProducerError>;
}

#[async_trait]
/// Reads producer history while validating and renewing a durable attachment.
pub(crate) trait AttachedStreamSegmentSource: Send + Sync {
    /// Counts committed producer events not yet represented in the consumer journal.
    async fn journal_lag_events(
        &self,
        handle: &DurableStreamHandle,
        after: Option<StreamOffset>,
    ) -> Result<usize, DurableStreamProducerError>;

    /// Reads currently available committed events under an active attachment.
    async fn read_attached_segment(
        &self,
        attachment: &StreamAttachmentKey,
        handle: &DurableStreamHandle,
        now_millis: u64,
        after: Option<StreamOffset>,
        through: Option<StreamOffset>,
    ) -> Result<Vec<CommittedProducerStreamEvent>, DurableStreamProducerError>;

    /// Waits for committed history beyond `after` while the attachment remains valid.
    async fn wait_for_attached_segment(
        &self,
        attachment: &StreamAttachmentKey,
        handle: &DurableStreamHandle,
        now_millis: u64,
        after: Option<StreamOffset>,
    ) -> Result<Vec<CommittedProducerStreamEvent>, DurableStreamProducerError>;
}

#[async_trait]
/// Durable lifecycle operations for producer-to-consumer history dependencies.
pub(crate) trait StreamAttachmentControl: Send + Sync {
    /// Records an attachment before the consumer begins depending on producer history.
    async fn prepare_attachment(
        &self,
        key: StreamAttachmentKey,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcome<StreamAttachmentView>, DurableStreamProducerError>;

    /// Makes a prepared attachment active and renews its lease.
    async fn activate_attachment(
        &self,
        key: StreamAttachmentKey,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcome<StreamAttachmentView>, DurableStreamProducerError>;

    /// Finalizes a transport attachment without cancelling the stream itself.
    async fn detach_attachment(
        &self,
        key: &StreamAttachmentKey,
    ) -> Result<StreamAttachmentView, DurableStreamProducerError>;

    /// Extends the lease of the same attachment epoch.
    async fn renew_attachment(
        &self,
        key: StreamAttachmentKey,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcome<StreamAttachmentView>, DurableStreamProducerError>;

    /// Durably removes the consumer's remaining dependency on producer history.
    async fn finalize_attachment(
        &self,
        key: StreamAttachmentKey,
        reason: StreamAttachmentFinalizationReason,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcome<StreamAttachmentView>, DurableStreamProducerError>;

    #[cfg(test)]
    async fn inspect_attachments(&self) -> Vec<StreamAttachmentView>;
}
