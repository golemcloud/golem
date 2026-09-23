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
mod fork;
mod index;
mod items;
pub(crate) mod metadata;
mod mutation;
mod probe;
mod publication;
mod registration;
mod routing;
mod session;
mod session_state;
mod terminals;
#[cfg(test)]
pub(crate) mod tests;

pub use catch_up::DurableCatchUpReader;
pub use items::StreamHead;
pub use metadata::{
    ProducerMetadataKey, ProducerMetadataRow, ProducerSessionMetadata, ProducerStreamMetadata,
};
pub use mutation::{StreamWriteAdmission, StreamWriteContext};
pub use probe::{
    ConsumerAttachmentStatus, ConsumerJournalInspection, DbDirectStreamAttachmentConsumerProbe,
    StreamAttachmentConsumerProbe,
};
use publication::CommittedEventRetention;
pub use registration::ResultStreamRegistration;
pub use routing::{RoutedAttachedStreamSegmentSource, RoutedStreamAttachmentControl};
pub use session_state::{
    CancellationWork, RecoveredMappings, RecoveryTopology, ResultMaterializationState,
    SessionControlMetadata,
};

use crate::durable_host::stream_bus::{
    DurableLiveStreamBus, DurableLiveStreamBusError, DurableLiveStreamEvent,
    DurableLiveStreamSubscription, PublicationReceipt, QueuedDurableEvent,
};
use crate::services::activity::{ActivityGate, spawn_with_activity};
use crate::services::oplog::{
    CommitLevel, DurableStreamOplogRecord, Oplog, OplogOps, OplogService, OplogServiceOps,
};
use crate::services::rpc::{DurableStreamReadError, Rpc};
use crate::services::worker::WorkerService;
use crate::services::worker_fork::lineage::StreamForkLineage;
use async_trait::async_trait;
use futures::FutureExt;
use golem_common::base_model::component::ComponentRevision;
use golem_common::base_model::durable_stream::{
    AttachedStreamSegmentRequest, AttachmentId, DEFAULT_LIVE_JOIN_BUFFER_SIZE,
    DURABLE_STREAM_FORMAT_VERSION, DurableStreamHandle, ExternalProducerId, InputStreamHighWater,
    LocalStreamId, LocalStreamReaderId, MAX_DURABLE_STREAM_ITEM_SIZE,
    MAX_DURABLE_STREAMS_PER_SESSION, MAX_NEW_STREAM_HANDLES_PER_VALUE,
    MAX_PACKED_U8_STREAM_ITEM_SIZE, MAX_STREAM_VALUE_TRAVERSAL_DEPTH,
    STREAM_ATTACHMENT_LEASE_TTL_MILLIS, SessionStreamRole, StreamAttachmentActivatedRecord,
    StreamAttachmentControlOperation, StreamAttachmentControlRequest,
    StreamAttachmentFinalizationReason, StreamAttachmentFinalizedRecord, StreamAttachmentKey,
    StreamAttachmentPreparedRecord, StreamBindingRecord, StreamCancelReason, StreamCancelRecord,
    StreamCancelRole, StreamCascadeDependentResult, StreamCascadeOutboxRecord, StreamEndRecord,
    StreamEndResult, StreamExternalProducerStateRecord, StreamId, StreamInvocationId,
    StreamItemsPayload, StreamItemsRecord, StreamOffset, StreamProducerDeletingRecord,
    StreamRecordReference, StreamRegisteredRecord, StreamRegistrationCoordinate,
    StreamRegistrationInvocation, StreamRegistrationRecordCoordinate, StreamSessionAttachedRecord,
    StreamSessionExpiryRefreshedRecord, StreamSessionFinishedRecord,
    StreamSessionInputHighWaterRecord, StreamSessionKey, StreamSessionMapping,
    StreamSessionMappingRecord, StreamSessionPreparedRecord, StreamSessionRecord, StreamSourceKind,
    StreamSourceUnavailableRecord, StreamTerminalAuthor, StreamTopologyPreparedRecord,
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
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use tokio::sync::{Mutex, MutexGuard, Notify, OwnedSemaphorePermit, Semaphore, oneshot};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Eq, PartialEq)]
/// Stable producer-side facts required to register a stream in an invocation session.
pub struct ProducerRegistrationRequest {
    pub coordinate: StreamRegistrationCoordinate,
    pub source_invocation: StreamRegistrationInvocation,
    pub entity_parent_start_index: Option<OplogIndex>,
    pub component_revision: ComponentRevision,
    pub element_schema_fingerprint: SchemaFingerprintV1,
    pub source_kind: StreamSourceKind,
    pub session_mapping: Option<StreamSessionMapping>,
}

#[derive(Clone, Debug, Eq, PartialEq, desert_rust::BinaryCodec)]
pub struct RegisteredStream {
    pub record: StreamRegisteredRecord,
    pub registration_oplog_index: OplogIndex,
    pub handle: DurableStreamHandle,
    pub coordinate: StreamRegistrationCoordinate,
    pub session_mapping: Option<StreamSessionMapping>,
}

fn qualify_local_stream(
    local_id: LocalStreamId,
    environment_id: EnvironmentId,
    producer: &AgentId,
    producer_fingerprint: AgentFingerprint,
) -> Result<StreamId, StreamStoreError> {
    StreamId::derive(environment_id, producer, producer_fingerprint, local_id.0)
        .map_err(|error| StreamStoreError::CorruptHistory(error.to_string()))
}

fn materialize_stream_reference(
    reference: &StreamRecordReference,
    environment_id: EnvironmentId,
    producer: &AgentId,
    producer_fingerprint: AgentFingerprint,
) -> Result<StreamId, StreamStoreError> {
    match reference {
        StreamRecordReference::Local(local_id) => {
            qualify_local_stream(*local_id, environment_id, producer, producer_fingerprint)
        }
        StreamRecordReference::Foreign(handle) => Ok(handle.stream_id),
    }
}

impl RegisteredStream {
    pub(crate) fn resolve(
        record: StreamRegisteredRecord,
        registration_oplog_index: OplogIndex,
        environment_id: EnvironmentId,
        producer: &AgentId,
        producer_fingerprint: AgentFingerprint,
    ) -> Result<Self, StreamStoreError> {
        let source_invocation = match &record.source_invocation {
            StreamRegistrationInvocation::Local(idempotency_key) => StreamInvocationId {
                callee_environment_id: environment_id,
                callee: producer.clone(),
                callee_fingerprint: producer_fingerprint,
                idempotency_key: idempotency_key.clone(),
            },
            StreamRegistrationInvocation::Remote(invocation) => invocation.clone(),
        };
        let session_mapping = record
            .session_role
            .map(|role| -> Result<_, StreamStoreError> {
                Ok(StreamSessionMapping {
                    session_key: source_invocation.clone(),
                    attachment_id: AttachmentId::primary(
                        source_invocation.callee_environment_id,
                        &source_invocation.callee,
                        &source_invocation.idempotency_key,
                    )
                    .map_err(|error| StreamStoreError::CorruptHistory(error.to_string()))?,
                    role,
                })
            })
            .transpose()?;
        let stream_id = StreamId::derive(
            environment_id,
            producer,
            producer_fingerprint,
            registration_oplog_index,
        )
        .map_err(|error| StreamStoreError::CorruptHistory(error.to_string()))?;
        let handle = DurableStreamHandle {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            stream_id,
            producer_environment_id: environment_id,
            producer: producer.clone(),
            expected_producer_fingerprint: producer_fingerprint,
            producer_generation: OplogIndex::NONE,
            source_invocation,
            component_revision: record.component_revision,
            element_schema_fingerprint: record.element_schema_fingerprint,
        };
        let coordinate = match &record.coordinate {
            StreamRegistrationRecordCoordinate::Root {
                invocation,
                root_kind,
                recursive_value_path,
            } => {
                let invocation_id = match invocation {
                    StreamRegistrationInvocation::Local(key) => StreamInvocationId {
                        callee_environment_id: environment_id,
                        callee: producer.clone(),
                        callee_fingerprint: producer_fingerprint,
                        idempotency_key: key.clone(),
                    },
                    StreamRegistrationInvocation::Remote(invocation) => invocation.clone(),
                };
                StreamRegistrationCoordinate::Root {
                    invocation_id,
                    root_kind: *root_kind,
                    recursive_value_path: recursive_value_path.clone(),
                }
            }
            StreamRegistrationRecordCoordinate::Nested {
                parent_stream,
                parent_producer_sequence,
                recursive_value_path,
            } => StreamRegistrationCoordinate::Nested {
                parent_stream_id: materialize_stream_reference(
                    parent_stream,
                    environment_id,
                    producer,
                    producer_fingerprint,
                )?,
                parent_producer_sequence: *parent_producer_sequence,
                recursive_value_path: recursive_value_path.clone(),
            },
        };
        Ok(Self {
            record,
            registration_oplog_index,
            handle,
            coordinate,
            session_mapping,
        })
    }

    pub(crate) fn issue(&self, generation: OplogIndex) -> DurableStreamHandle {
        let mut handle = self.handle.clone();
        handle.producer_generation = generation;
        handle
    }

    pub(crate) fn accepts(&self, handle: &DurableStreamHandle, generation: OplogIndex) -> bool {
        handle.producer_generation == generation && self.issue(generation) == *handle
    }
}

impl std::ops::Deref for RegisteredStream {
    type Target = StreamRegisteredRecord;

    fn deref(&self) -> &Self::Target {
        &self.record
    }
}

impl std::ops::DerefMut for RegisteredStream {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.record
    }
}

impl std::borrow::Borrow<StreamRegisteredRecord> for RegisteredStream {
    fn borrow(&self) -> &StreamRegisteredRecord {
        &self.record
    }
}

#[derive(Clone)]
/// Describes whether a nested stream is created here or forwards an existing durable handle.
pub enum NestedStreamWrite {
    Register(ProducerRegistrationRequest),
    Forward(DurableStreamHandle),
}

/// Selects a newly registered or already durable source for an output mapping.
pub enum ProducerOutputSource {
    New(ProducerRegistrationRequest),
    Existing(DurableStreamHandle),
}

/// Associates a transport output slot with its durable producer source.
pub struct ProducerOutputRegistration {
    pub transport_stream_id: u64,
    pub source: ProducerOutputSource,
    pub cancellation_epoch: Option<u64>,
}

/// A cancellation whose oplog record is durable but whose postcommit publication is pending.
pub(crate) struct PendingCommittedCancellation {
    stream_id: StreamId,
    sequence: u64,
    role: StreamCancelRole,
    reason: StreamCancelReason,
    publication: PublicationReceipt,
    event: CommittedProducerStreamEvent,
    outcome: Result<ProducerWriteOutcome<StreamOffset>, StreamStoreError>,
}

#[derive(Clone, Debug, Eq, PartialEq, desert_rust::BinaryCodec)]
/// Guest-visible payload reconstructed from a committed producer stream record.
pub enum CommittedProducerStreamEventPayload {
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
pub struct CommittedProducerStreamEvent {
    pub stream_id: StreamId,
    pub producer_sequence: u64,
    pub offset: StreamOffset,
    pub packed_u8_batch_end: Option<StreamOffset>,
    pub terminal_author: Option<StreamTerminalAuthor>,
    pub nested_handles: Vec<DurableStreamHandle>,
    pub nested_references: Vec<StreamRecordReference>,
    pub payload: CommittedProducerStreamEventPayload,
}

impl CommittedProducerStreamEvent {
    /// Returns whether this event permanently closes the stream.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.payload,
            CommittedProducerStreamEventPayload::End(_)
                | CommittedProducerStreamEventPayload::Cancel { .. }
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Reports a mutation result and whether it was recovered from existing durable state.
pub struct ProducerWriteOutcome<T> {
    pub value: T,
    pub replayed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, desert_rust::BinaryCodec)]
/// A bounded durable read plus the cursor and terminal state needed to continue it.
pub struct StreamHandleReadResult {
    pub events: Vec<CommittedProducerStreamEvent>,
    pub next_offset: Option<StreamOffset>,
    pub head_offset: Option<StreamOffset>,
    pub closed: bool,
    pub cancelled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Identity and monotonic write position of an external producer attachment.
pub struct ExternalProducer {
    pub id: ExternalProducerId,
    pub epoch: u64,
    pub sequence: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Durable admission result for an externally supplied input event.
pub enum ExternalAppendOutcome {
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

#[derive(Clone, Debug)]
pub(crate) struct ExportForkStreamSnapshot {
    pub(crate) horizon: OplogIndex,
    pub(crate) registration_index: OplogIndex,
    pub(crate) batches: Vec<(OplogIndex, u32)>,
    pub(crate) terminal: Option<StreamOffset>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Contract and persistence failures detected by the durable stream store.
pub enum StreamStoreError {
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
    ProducerDeleting,
    ConsumerDeleting,
    ConsumerJournalAdvanced,
    DeletionBlocked(Vec<StreamAttachmentKey>),
    CorruptHistory(String),
    Oplog(String),
    RecoveryRequired,
    LiveBus(DurableLiveStreamBusError),
}

impl std::fmt::Display for StreamStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for StreamStoreError {}

impl From<StreamStoreError> for String {
    fn from(error: StreamStoreError) -> Self {
        error.to_string()
    }
}

impl StreamStoreError {
    /// Formats the dependent attachment identities that currently block deletion.
    pub fn deletion_blocked_evidence(&self) -> Option<String> {
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

impl From<DurableLiveStreamBusError> for StreamStoreError {
    fn from(value: DurableLiveStreamBusError) -> Self {
        match value {
            DurableLiveStreamBusError::Retired => Self::RecoveryRequired,
            value => Self::LiveBus(value),
        }
    }
}

#[derive(Clone, Default)]
struct ProducerStreamIndex {
    fork_lineage: StreamForkLineage,
    applied_fork_cuts: BTreeMap<OplogIndex, Arc<HashSet<StreamId>>>,
    loaded_metadata: HashSet<metadata::ProducerMetadataKey>,
    registrations: HashMap<StreamId, RegisteredStream>,
    local_stream_ids: HashMap<LocalStreamId, StreamId>,
    entity_parent_start_indices: HashMap<StreamId, Option<OplogIndex>>,
    coordinates: HashMap<StreamRegistrationCoordinate, StreamId>,
    streams: HashMap<StreamId, IndexedProducerStream>,
    stream_sessions: HashMap<StreamId, StreamSessionKey>,
    stream_roles: HashMap<StreamId, SessionStreamRole>,
    session_stream_mappings:
        HashMap<StreamSessionKey, HashSet<(StreamRecordReference, SessionStreamRole)>>,
    session_stream_counts: HashMap<StreamSessionKey, usize>,
    session_count: u64,
    session_pages: HashMap<u64, Vec<StreamSessionKey>>,
    session_consumer_streams: HashMap<StreamSessionKey, HashSet<LocalStreamReaderId>>,
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
    consumer_journals: HashMap<(StreamSessionKey, LocalStreamReaderId), IndexedConsumerJournal>,
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
    source_unavailable: Option<StreamOffset>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ProducerJournalSummary {
    event_count: u64,
    last_offset: Option<StreamOffset>,
    terminal: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Folded count, cursor, and terminal state of a consumer journal.
pub struct ConsumerJournalSummary {
    pub event_count: u64,
    pub last_offset: Option<StreamOffset>,
    pub terminal: bool,
    pub source_unavailable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, desert_rust::BinaryCodec)]
enum IndexedStreamAttachmentState {
    Prepared {
        prepared_at_millis: u64,
        lease_expires_at_millis: u64,
    },
    Active {
        activated_at_millis: u64,
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
pub enum StreamAttachmentState {
    Prepared,
    Active,
    Finalized(StreamAttachmentFinalizationReason),
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Current attachment phase and its preparation deadline, if still prepared.
pub struct StreamAttachmentView {
    pub key: StreamAttachmentKey,
    pub state: StreamAttachmentState,
    pub lease_expires_at_millis: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Durable evidence used to explain or perform producer deletion.
pub struct StreamDeletionDiagnostics {
    pub deleting: bool,
    pub attachments: Vec<StreamAttachmentView>,
    pub cascade_completed: Vec<(StreamAttachmentKey, StreamCascadeDependentResult)>,
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
    last_item_offset: Option<StreamOffset>,
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

/// Owns one agent's persisted stream journal, indexes, and disposable live publication state.
/// Session-specific transport and consumer behavior belongs to `StreamSession` instead.
pub struct DurableStreamStore {
    self_weak: std::sync::Weak<Self>,
    oplog: Arc<dyn Oplog>,
    pub(crate) fork_lineage: Arc<StreamForkLineage>,
    applied_fork_cuts: BTreeMap<OplogIndex, Arc<HashSet<StreamId>>>,
    commit: DurableStreamCommit,
    worker_tasks: std::sync::OnceLock<crate::worker::tasks::WorkerTasks>,
    control_metadata_provider: std::sync::OnceLock<(Arc<dyn WorkerService>, AgentMode)>,
    environment_id: EnvironmentId,
    producer: AgentId,
    producer_fingerprint: AgentFingerprint,
    producer_generation: OplogIndex,
    index: Mutex<ProducerStreamIndex>,
    poisoned: AtomicBool,
    retirement: CancellationToken,
    durable_activity: Arc<ActivityGate>,
    mutations: mutation::MutationQueue,
    publication_gate: Arc<Mutex<()>>,
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
    reconcilable_attachment_count: AtomicU64,
    open_stream_count: AtomicUsize,
    live_join_capacity: usize,
    session_records_changed: Notify,
    session_locks:
        std::sync::Mutex<HashMap<StreamSessionKey, std::sync::Weak<tokio::sync::Mutex<()>>>>,
}

impl DurableStreamStore {
    pub(crate) fn qualify_session(
        &self,
        session: &StreamRegistrationInvocation,
    ) -> StreamSessionKey {
        session.qualify(
            self.environment_id,
            &self.producer,
            self.producer_fingerprint,
        )
    }

    pub(crate) fn owns_current_activity(&self) -> bool {
        self.durable_activity.is_current()
    }

    /// Present only when this store applied the marker during reconstruction.
    pub(crate) fn applied_fork_cut(
        &self,
        index: OplogIndex,
    ) -> Option<(
        &golem_common::model::durable_stream::StreamForkCutRecord,
        &HashSet<StreamId>,
    )> {
        let retained = self.applied_fork_cuts.get(&index)?;
        self.fork_lineage
            .cuts()
            .iter()
            .find_map(|(marker, cut)| (*marker == index).then_some((cut, retained.as_ref())))
    }

    pub(crate) fn set_worker_tasks(&self, tasks: crate::worker::tasks::WorkerTasks) {
        assert!(self.worker_tasks.set(tasks).is_ok());
    }

    pub(crate) fn tasks(&self) -> &crate::worker::tasks::WorkerTasks {
        self.worker_tasks
            .get_or_init(|| self.oplog.task_owner().cloned().unwrap_or_default())
    }

    pub(crate) async fn load(
        oplog: Arc<dyn Oplog>,
        environment_id: EnvironmentId,
        producer: AgentId,
        producer_fingerprint: AgentFingerprint,
        live_join_capacity: Option<usize>,
    ) -> Result<Arc<Self>, StreamStoreError> {
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
    ) -> Result<Arc<Self>, StreamStoreError> {
        let live_join_capacity = live_join_capacity.unwrap_or(DEFAULT_LIVE_JOIN_BUFFER_SIZE);
        DurableLiveStreamBus::<CommittedProducerStreamEvent>::new(live_join_capacity)?;
        let index = Self::read_complete_index(
            oplog.as_ref(),
            environment_id,
            &producer,
            producer_fingerprint,
            oplog.current_oplog_index().await,
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
        current_index: OplogIndex,
    ) -> Result<ProducerStreamIndex, StreamStoreError> {
        let owner = OwnedAgentId::new(environment_id, producer);
        let lineage =
            StreamForkLineage::load_at_horizon(oplog, &owner, producer_fingerprint, current_index)
                .await
                .map_err(StreamStoreError::CorruptHistory)?;
        let mut index = ProducerStreamIndex::default();
        let mut pending_nested_registrations = Vec::new();
        let mut covered = OplogIndex::NONE;
        while covered < current_index {
            let count = (current_index.as_u64() - covered.as_u64()).min(1024);
            let entries = oplog.read_exact(covered.next(), count).await;
            for (oplog_index, entry) in entries {
                covered = oplog_index;
                if lineage.deleted_regions().is_in_deleted_region(oplog_index) {
                    continue;
                }
                match entry {
                    OplogEntry::StreamRegistered {
                        entity_parent_start_index,
                        record,
                        ..
                    } => {
                        let record = oplog
                            .download_payload(record)
                            .await
                            .map_err(StreamStoreError::Oplog)?;
                        if matches!(
                            &record.coordinate,
                            StreamRegistrationRecordCoordinate::Nested { .. }
                        ) {
                            pending_nested_registrations.push((
                                oplog_index,
                                entity_parent_start_index,
                                record,
                            ));
                        } else {
                            if !pending_nested_registrations.is_empty() {
                                return Err(StreamStoreError::CorruptHistory(
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
                            .map_err(StreamStoreError::Oplog)?;
                        index.apply_item_batch(
                            oplog_index,
                            entity_parent_start_index,
                            std::mem::take(&mut pending_nested_registrations),
                            record,
                            environment_id,
                            producer,
                            producer_fingerprint,
                            lineage
                                .cuts()
                                .iter()
                                .rev()
                                .find_map(|(marker, cut)| cut.revert.is_some().then_some(*marker))
                                .unwrap_or(OplogIndex::NONE),
                        )?;
                    }
                    OplogEntry::StreamEnd {
                        entity_parent_start_index,
                        record,
                        ..
                    } => {
                        if !pending_nested_registrations.is_empty() {
                            return Err(StreamStoreError::CorruptHistory(
                                "nested registration batch is missing its enclosing item"
                                    .to_string(),
                            ));
                        }
                        let record = oplog
                            .download_payload(record)
                            .await
                            .map_err(StreamStoreError::Oplog)?;
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
                            return Err(StreamStoreError::CorruptHistory(
                                "nested registration batch is missing its enclosing item"
                                    .to_string(),
                            ));
                        }
                        let record = oplog
                            .download_payload(record)
                            .await
                            .map_err(StreamStoreError::Oplog)?;
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
                            return Err(StreamStoreError::CorruptHistory(
                                "nested registration batch is missing its enclosing item"
                                    .to_string(),
                            ));
                        }
                        let record = oplog
                            .download_payload(record)
                            .await
                            .map_err(StreamStoreError::Oplog)?;
                        if lineage.resets_control(oplog_index, &record) {
                            continue;
                        }
                        if let StreamSessionRecord::ForkCut(record) = &record {
                            let expected = lineage
                                .cuts()
                                .iter()
                                .find(|(index, _)| *index == oplog_index)
                                .map(|(_, record)| record);
                            if expected != Some(record) {
                                return Err(StreamStoreError::CorruptHistory(
                                    "fork marker differs from the validated lineage".into(),
                                ));
                            }
                            let retained_items = index.resolve_fork_prefix(record)?;
                            let changes = index.apply_fork_cut(record, retained_items)?;
                            index
                                .applied_fork_cuts
                                .insert(oplog_index, Arc::new(changes.changed_streams));
                            continue;
                        }
                        index.apply_session_references(
                            entity_parent_start_index,
                            &record,
                            environment_id,
                            producer,
                            producer_fingerprint,
                        )?;
                        index.apply_result_offset(
                            oplog_index,
                            &record,
                            environment_id,
                            producer,
                            producer_fingerprint,
                        );
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
                            index.apply_finished(
                                record,
                                environment_id,
                                producer,
                                producer_fingerprint,
                            )?;
                        }
                    }
                    _ => {
                        if !pending_nested_registrations.is_empty() {
                            return Err(StreamStoreError::CorruptHistory(
                                "nested registration batch is missing its enclosing item"
                                    .to_string(),
                            ));
                        }
                    }
                }
            }
        }
        if !pending_nested_registrations.is_empty() {
            return Err(StreamStoreError::CorruptHistory(
                "nested registration batch is missing its enclosing item".to_string(),
            ));
        }

        index.fork_lineage = lineage;
        Ok(index)
    }

    fn from_index(
        oplog: Arc<dyn Oplog>,
        environment_id: EnvironmentId,
        producer: AgentId,
        producer_fingerprint: AgentFingerprint,
        live_join_capacity: usize,
        commit: DurableStreamCommit,
        mut index: ProducerStreamIndex,
    ) -> Result<Arc<Self>, StreamStoreError> {
        let mut buses = BTreeMap::new();
        for (stream_id, stream) in &index.streams {
            let bus = Arc::new(DurableLiveStreamBus::from_committed_high_water(
                live_join_capacity,
                stream.last_offset,
            )?);
            buses.insert(*stream_id, bus);
        }

        let open_stream_count = index.open_streams;
        let reconcilable_attachment_count =
            if index.loaded_metadata.contains(&ProducerMetadataKey::Global) {
                index.active_attachment_count
            } else {
                index
                    .attachments
                    .values()
                    .filter(|attachment| {
                        !matches!(
                            attachment.state,
                            IndexedStreamAttachmentState::Finalized { .. }
                        )
                    })
                    .count() as u64
            };
        crate::metrics::durable_stream::add_open_streams(open_stream_count);
        if !index.streams.is_empty() {
            tracing::debug!(
                recovered_streams = index.streams.len(),
                recovered_open_streams = open_stream_count,
                "Durable stream producer index recovered"
            );
        }
        let producer_generation = index
            .fork_lineage
            .cuts()
            .iter()
            .rev()
            .find_map(|(marker, cut)| cut.revert.is_some().then_some(*marker))
            .unwrap_or(OplogIndex::NONE);
        let fork_lineage = Arc::new(std::mem::take(&mut index.fork_lineage));
        let applied_fork_cuts = std::mem::take(&mut index.applied_fork_cuts);
        Ok(Arc::new_cyclic(|self_weak| Self {
            self_weak: self_weak.clone(),
            oplog,
            fork_lineage,
            applied_fork_cuts,
            commit,
            worker_tasks: std::sync::OnceLock::new(),
            control_metadata_provider: std::sync::OnceLock::new(),
            environment_id,
            producer,
            producer_fingerprint,
            producer_generation,
            index: Mutex::new(index),
            poisoned: AtomicBool::new(false),
            retirement: CancellationToken::new(),
            durable_activity: ActivityGate::new(),
            mutations: mutation::MutationQueue::new(),
            publication_gate: Arc::new(Mutex::new(())),
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
            reconcilable_attachment_count: AtomicU64::new(reconcilable_attachment_count),
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
    pub fn environment_id(&self) -> EnvironmentId {
        self.environment_id
    }

    /// Returns the agent whose oplog owns these streams.
    pub fn agent_id(&self) -> &AgentId {
        &self.producer
    }

    /// Returns the durable producer identity that fences recreated agents.
    pub fn fingerprint(&self) -> AgentFingerprint {
        self.producer_fingerprint
    }

    pub(crate) fn generation(&self) -> OplogIndex {
        self.producer_generation
    }
}

impl Drop for DurableStreamStore {
    fn drop(&mut self) {
        self.terminal_progress.notify_one();
        crate::metrics::durable_stream::remove_open_streams(
            self.open_stream_count.load(Ordering::Relaxed),
        );
    }
}

#[async_trait]
/// Reads committed producer history without relying on a resident live subscription.
pub trait StreamSegmentSource: Send + Sync {
    /// Reads committed events after `after`, optionally stopping at `through`.
    async fn read_segment(
        &self,
        handle: &DurableStreamHandle,
        after: Option<StreamOffset>,
        through: Option<StreamOffset>,
    ) -> Result<Vec<CommittedProducerStreamEvent>, StreamStoreError>;
}

#[async_trait]
/// Reads producer history while validating a durable attachment.
pub trait AttachedStreamSegmentSource: Send + Sync {
    /// Counts committed producer events not yet represented in the consumer journal.
    async fn journal_lag_events(
        &self,
        handle: &DurableStreamHandle,
        after: Option<StreamOffset>,
    ) -> Result<usize, StreamStoreError>;

    /// Reads currently available committed events under an active attachment.
    async fn read_attached_segment(
        &self,
        attachment: &StreamAttachmentKey,
        handle: &DurableStreamHandle,
        now_millis: u64,
        after: Option<StreamOffset>,
        through: Option<StreamOffset>,
    ) -> Result<Vec<CommittedProducerStreamEvent>, StreamStoreError>;

    /// Waits for committed history beyond `after` while the attachment remains valid.
    async fn wait_for_attached_segment(
        &self,
        attachment: &StreamAttachmentKey,
        handle: &DurableStreamHandle,
        now_millis: u64,
        after: Option<StreamOffset>,
    ) -> Result<Vec<CommittedProducerStreamEvent>, StreamStoreError>;
}

#[async_trait]
/// Durable lifecycle operations for producer-to-consumer history dependencies.
pub trait StreamAttachmentControl: Send + Sync {
    /// Records an attachment before the consumer begins depending on producer history.
    async fn prepare_attachment(
        &self,
        key: StreamAttachmentKey,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcome<StreamAttachmentView>, StreamStoreError>;

    /// Makes a prepared attachment active until it is explicitly finalized or superseded.
    async fn activate_attachment(
        &self,
        key: StreamAttachmentKey,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcome<StreamAttachmentView>, StreamStoreError>;

    /// Finalizes a transport attachment without cancelling the stream itself.
    async fn detach_attachment(
        &self,
        key: &StreamAttachmentKey,
    ) -> Result<StreamAttachmentView, StreamStoreError>;

    /// Durably removes the consumer's remaining dependency on producer history.
    async fn finalize_attachment(
        &self,
        key: StreamAttachmentKey,
        reason: StreamAttachmentFinalizationReason,
        now_millis: u64,
    ) -> Result<ProducerWriteOutcome<StreamAttachmentView>, StreamStoreError>;

    #[cfg(test)]
    async fn inspect_attachments(&self) -> Vec<StreamAttachmentView>;
}
