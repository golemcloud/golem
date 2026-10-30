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

use crate::durable_host::concurrent::{
    DropEvent, LiveCallPermit, cancel_dropped_durable_input_access,
    finish_prepared_access_to_live_at,
};
use crate::durable_host::durability::{
    ClassifiedHostError, DurabilityHost, DurableCallTrapContext, HostFailureKind,
    InFunctionRetryHost,
};
use crate::durable_host::durable_stream::{
    AttachedStreamSegmentSource, CommittedProducerStreamEvent, CommittedProducerStreamEventPayload,
    ConsumerAttachmentStatus, DurableCatchUpReader, DurableStreamStore, NestedStreamWrite,
    ProducerOutputRegistration, ProducerOutputSource, ProducerRegistrationRequest,
    RecoveredMappings, ResultMaterializationState, ResultStreamRegistration,
    RoutedAttachedStreamSegmentSource, RoutedStreamAttachmentControl, SessionControlMetadata,
    SessionError, StreamAttachmentConsumerProbe, StreamAttachmentControl, StreamSegmentSource,
    StreamStoreError, StreamWriteAdmission, StreamWriteContext,
};
use crate::durable_host::input_observation::{
    EnqueueInputObservation, InputObservationDelivery, InputReadObserver as StreamReadObserver,
};
use crate::durable_host::replay_state::ReplayState;
use crate::durable_host::schema_value_stream::StoreValueResolver;
use crate::durable_host::stream_bus::{LiveStreamEventPayload, LiveStreamReceiveError};
use crate::durable_host::stream_session::{
    decode_recursive_stream_value, decode_recursive_stream_value_with_schema,
    encode_recursive_stream_value_with_schema, preflight_proto_recursive_stream_value,
    preflight_recursive_stream_value, remap_recursive_stream_references,
};
use crate::durable_host::stream_transport::{LiveStreamEndpoint, SourceLifecycle};
use crate::durable_host::tail_work::TailActivity;
use crate::durable_host::{BeginReplayToLive, DurableWorkerCtx, DurableWorkerCtxView};
use crate::preview2::golem::tool::host::ByteStreamFailure;
use crate::services::oplog::{Oplog, OplogOps};
use crate::services::rpc::Rpc;
use crate::worker::suspension::{ExternalActivity, RuntimeSource, RuntimeStore};
use crate::workerctx::WorkerCtx;
use futures::FutureExt;
use futures::future::{BoxFuture, Shared, try_join_all};
use golem_api_grpc::proto::golem::worker::{
    DurableStreamHandle as ProtoDurableStreamHandle, DurableStreamMapping,
    InputStreamHighWater as ProtoInputStreamHighWater, InvocationResponse, OutputStreamEnd,
    OutputStreamError, OutputStreamItem, StreamCancel, StreamInvocationIdentity, StreamMappingRole,
    ToolByteStreamRole as ProtoToolByteStreamRole, invocation_response,
};
use golem_common::base_model::durable_stream::{
    AttachmentId, AttemptId, DURABLE_STREAM_FORMAT_VERSION, DurableStreamHandle,
    InputStreamHighWater, LocalStreamReaderId, MAX_NEW_STREAM_HANDLES_PER_VALUE,
    MAX_PACKED_U8_STREAM_ITEM_SIZE, STREAM_CONSUMER_RECORD_FORMAT_VERSION, SessionStreamRole,
    StreamAttachmentKey, StreamBindingRecord, StreamCallerAttemptRecord, StreamCancelReason,
    StreamCancelRole, StreamConsumerCancelAppliedRecord, StreamConsumerCancelIntentRecord,
    StreamConsumerItemValueRecord, StreamConsumerObservation, StreamConsumerTerminal,
    StreamConsumerTerminalRecord, StreamEndResult, StreamInvocationId, StreamItemsPayload,
    StreamOffset, StreamReaderForwardDestination, StreamReaderForwardIntentRecord,
    StreamReaderForwardPublication, StreamRecordReference, StreamRegistrationCoordinate,
    StreamRegistrationInvocation, StreamResumeOperation, StreamRootKind,
    StreamSessionDetachedRecord, StreamSessionInvocationResultRecord, StreamSessionKey,
    StreamSessionMapping, StreamSessionMappingRecord, StreamSessionMappingUpdateRecord,
    StreamSessionRecord, StreamSessionResumeAttemptRecord, StreamSlotTombstonedRecord,
    StreamSourceKind, StreamTopologyActivatedRecord, StreamTopologyPreparedRecord,
    StreamValuePathStep, ToolByteStreamRole,
};
use golem_common::base_model::oplog::OplogEntry;
use golem_common::model::Timestamp;
use golem_common::model::entity::OwnerRuntime;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::oplog::payload::OplogPayload;
use golem_common::model::oplog::payload::types::SerializableToolAdmissionRefusal;
use golem_schema::proto::golem::schema::SchemaValue as ProtoSchemaValue;
use golem_schema::schema::wit::{encode_value_with_streams, wire};
use golem_schema::schema::{SchemaFingerprintV1, SchemaGraph, SchemaType, schema_fingerprint_v1};
use golem_schema::schema::{SchemaValue, SchemaValueStream, TypedSchemaValue};
use golem_service_base::error::worker_executor::{InterruptKind, WorkerExecutorError};
use golem_service_base::model::auth::AuthCtx;
use prost::Message;
use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet, VecDeque};
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, mpsc, oneshot};
use wasmtime::component::{
    Accessor, Destination, HasSelf, RuntimeActivityId, StreamProducer, StreamResult,
};
use wasmtime::{AsContextMut, StoreContextMut};

mod attachment;
mod cancellation;
mod forwarding;
mod input;
mod input_producer;
mod input_receive;
mod output;
mod output_replay;

pub(crate) use forwarding::strip_typed_streams;
use forwarding::{
    collect_stream_paths, forwarded_durable_input_reference, stream_cancel_reason_to_proto,
    stream_element_schema, validate_forwarded_durable_input_schemas,
};
pub use forwarding::{
    durable_stream_mapping_from_proto, durable_stream_mapping_to_proto, strip_streams,
};
#[cfg(test)]
use input_producer::durable_stream_cancel_error;
#[cfg(test)]
use input_receive::await_first_durable_source;

const PACKED_U8_OUTPUT_FLUSH_DELAY: Duration = Duration::from_millis(50);

/// Durable attachment authority currently recorded for a stream session.
pub struct AuthoritativeAttachmentState {
    pub epoch: u64,
    pub attempt_id: AttemptId,
    pub attached: bool,
}

struct MaterializedResult {
    value: SchemaValue,
    drains: Vec<PendingOwnedStreamDrain>,
}

struct OutputReplay {
    mappings: Vec<StreamSessionMappingRecord>,
    terminal_cursor: bool,
}

#[async_trait::async_trait]
/// Commits consumer journal entries and queries their durable completion boundary.
pub trait DurableStreamConsumerJournal: Send + Sync {
    /// Makes previously appended consumer observations recoverable.
    async fn commit(&self) -> Result<(), SessionError>;
    /// Returns the committed session-finished index, if present.
    async fn committed_finished_index(
        &self,
        session: &StreamSessionKey,
    ) -> Result<Option<OplogIndex>, SessionError>;
}

#[derive(Clone)]
/// Runs one durable Stream Session's disposable mappings and transport collaborators.
/// Persisted stream records and indexes remain owned by `DurableStreamStore`.
pub struct StreamSession {
    pub(crate) producer: Arc<DurableStreamStore>,
    pub(crate) oplog: Arc<dyn Oplog>,
    pub(crate) session_reference: StreamRegistrationInvocation,
    pub(crate) session_key: StreamSessionKey,
    consumer_invocation: StreamInvocationId,
    bindings: Arc<RwLock<HashMap<u64, StreamBindingRecord>>>,
    mappings: Arc<RwLock<HashMap<u64, StreamSessionMappingRecord>>>,
    input_schema: Option<Arc<DurableInputSchema>>,
    rpc: Option<Arc<dyn Rpc>>,
    consumer_journal: Option<Arc<dyn DurableStreamConsumerJournal>>,
    auth_ctx: Option<Arc<AuthCtx>>,
    runtime_source: Option<RuntimeSource>,
    require_root_attachment_before_production: bool,
    next_transport_stream_id: Arc<AtomicU64>,
    session_lock: Arc<Mutex<()>>,
    attachment_epoch: u64,
    attachment_attempt_id: Option<AttemptId>,
    entity_parent_start_index: Option<OplogIndex>,
    recovered_mappings_through: Arc<Mutex<OplogIndex>>,
    control_metadata: Arc<Mutex<SessionControlMetadata>>,
    response_lease: Option<Arc<crate::worker::EphemeralResponseLease>>,
}

struct DurableInputSchema {
    graph: Arc<SchemaGraph>,
    component_revision: golem_common::model::component::ComponentRevision,
    element_types: RwLock<HashMap<u64, SchemaType>>,
}

struct OutputDrainRegistration {
    producer: Arc<DurableStreamStore>,
    stream_id: golem_common::model::StreamId,
    registration_id: u64,
    lifecycle: Arc<SourceLifecycle>,
}

struct PendingOwnedStreamDrain {
    handle: DurableStreamHandle,
    endpoint: LiveStreamEndpoint,
    element_type: SchemaType,
    role: SessionStreamRole,
    activity: Option<ExternalActivity>,
}

/// An output already registered and drained by this session before result publication.
pub(crate) struct RegisteredOutputStream {
    pub transport_stream_id: u64,
    pub role: SessionStreamRole,
}

/// A stream-bearing value whose references name its binding-local transport mappings.
#[derive(Debug, PartialEq)]
pub struct SessionValue {
    pub value: ProtoSchemaValue,
    pub mappings: Vec<StreamSessionMappingRecord>,
}

impl StreamSession {
    /// Creates session runtime state from its durable identity and known mappings.
    pub fn new(
        producer: Arc<DurableStreamStore>,
        oplog: Arc<dyn Oplog>,
        session_reference: StreamRegistrationInvocation,
        mappings: impl IntoIterator<Item = StreamSessionMappingRecord>,
    ) -> Self {
        let session_key = producer.qualify_session(&session_reference);
        let session_lock = producer.session_lock(&session_key);
        let mappings = mappings
            .into_iter()
            .map(|mapping| (mapping.transport_stream_id, mapping))
            .collect::<HashMap<_, _>>();
        let bindings = mappings
            .iter()
            .map(|(id, mapping)| (*id, StreamBindingRecord::foreign(mapping)))
            .collect();
        let next_transport_stream_id = mappings
            .keys()
            .copied()
            .max()
            .map(|id| id.saturating_add(1))
            .unwrap_or_default();
        let attachment_epoch = producer.attachment_epoch_floor();
        Self {
            producer,
            oplog,
            session_reference,
            consumer_invocation: session_key.clone(),
            session_key,
            bindings: Arc::new(RwLock::new(bindings)),
            mappings: Arc::new(RwLock::new(mappings)),
            input_schema: None,
            rpc: None,
            consumer_journal: None,
            auth_ctx: None,
            runtime_source: None,
            require_root_attachment_before_production: false,
            next_transport_stream_id: Arc::new(AtomicU64::new(next_transport_stream_id)),
            session_lock,
            attachment_epoch,
            attachment_attempt_id: None,
            entity_parent_start_index: None,
            recovered_mappings_through: Arc::new(Mutex::new(OplogIndex::NONE)),
            control_metadata: Arc::new(Mutex::new(SessionControlMetadata::default())),
            response_lease: None,
        }
    }

    /// Opens session runtime state from persisted owner-relative bindings.
    pub async fn open(
        producer: Arc<DurableStreamStore>,
        oplog: Arc<dyn Oplog>,
        session_reference: StreamRegistrationInvocation,
        bindings: impl IntoIterator<Item = StreamBindingRecord>,
    ) -> Result<Self, SessionError> {
        let bindings = bindings.into_iter().collect::<Vec<_>>();
        let mappings = producer
            .materialize_bindings(&bindings)
            .await
            .map_err(SessionError::from)?;
        let session = Self::new(producer, oplog, session_reference, mappings);
        *session
            .bindings
            .write()
            .expect("durable stream binding lock poisoned") = bindings
            .into_iter()
            .map(|binding| (binding.transport_stream_id, binding))
            .collect();
        Ok(session)
    }

    /// Retains an ephemeral response only for the lifetime of this session runtime.
    pub fn with_response_lease(
        mut self,
        lease: Option<Arc<crate::worker::EphemeralResponseLease>>,
    ) -> Self {
        self.response_lease = lease;
        self
    }

    /// Returns the resident ephemeral-response lease, if one is held.
    pub fn response_lease(&self) -> Option<Arc<crate::worker::EphemeralResponseLease>> {
        self.response_lease.clone()
    }

    /// Binds this runtime to one attachment epoch and attempt.
    pub fn with_attachment(mut self, epoch: u64, attempt_id: AttemptId) -> Self {
        self.recovered_mappings_through = Arc::new(Mutex::new(OplogIndex::NONE));
        self.attachment_epoch = epoch;
        self.attachment_attempt_id = Some(attempt_id);
        self
    }

    /// Sets the invocation identity attributed to consumer journal records.
    pub fn with_consumer_invocation(mut self, consumer_invocation: StreamInvocationId) -> Self {
        self.recovered_mappings_through = Arc::new(Mutex::new(OplogIndex::NONE));
        self.consumer_invocation = consumer_invocation;
        self
    }

    /// Attributes session records to an entity call in the owner's oplog.
    pub fn with_entity_parent_start_index(
        mut self,
        entity_parent_start_index: Option<OplogIndex>,
    ) -> Self {
        self.entity_parent_start_index = entity_parent_start_index;
        self
    }

    /// Enables routed producer attachment and segment operations.
    pub fn with_rpc(mut self, rpc: Arc<dyn Rpc>) -> Self {
        self.rpc = Some(rpc);
        self
    }

    /// Installs the journal whose commit makes guest observations durable.
    pub fn with_consumer_journal(
        mut self,
        consumer_journal: Arc<dyn DurableStreamConsumerJournal>,
    ) -> Self {
        self.consumer_journal = Some(consumer_journal);
        self
    }

    /// Commits appended consumer observations before they are returned to the guest.
    pub async fn commit_consumer_journal(&self) -> Result<(), SessionError> {
        self.consumer_journal
            .as_ref()
            .ok_or_else(|| "durable stream consumer journal commit is unavailable".to_string())?
            .commit()
            .await
    }

    /// Preserves the original consumer authorization for routed source operations.
    pub fn with_auth_ctx(mut self, auth_ctx: Arc<AuthCtx>) -> Self {
        self.auth_ctx = Some(auth_ctx);
        self
    }

    pub(crate) fn with_runtime_source(mut self, source: Option<RuntimeSource>) -> Self {
        self.runtime_source = source;
        self
    }

    fn drain_activity(&self, endpoint: &LiveStreamEndpoint) -> Option<ExternalActivity> {
        self.runtime_source
            .as_ref()
            .map(RuntimeSource::external_activity)
            .or_else(|| endpoint.external_activity())
    }

    /// Requires durable root attachment activation before open output production.
    pub fn require_root_attachment_before_production(mut self) -> Self {
        self.require_root_attachment_before_production = true;
        self
    }

    fn allocate_transport_stream_id(&self) -> Result<u64, SessionError> {
        self.next_transport_stream_id
            .try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(1)
            })
            .map_err(|_| "durable transport stream id overflow".to_string())
            .map_err(SessionError::from)
    }

    /// Pins input element schemas and component revision for this durable session.
    pub fn with_input_schema(
        mut self,
        graph: Arc<SchemaGraph>,
        component_revision: golem_common::model::component::ComponentRevision,
        element_types: impl IntoIterator<Item = (u64, SchemaType)>,
    ) -> Self {
        self.input_schema = Some(Arc::new(DurableInputSchema {
            graph,
            component_revision,
            element_types: RwLock::new(element_types.into_iter().collect()),
        }));
        self
    }

    /// Resolves a transport-local stream ID to its durable handle.
    pub fn handle(&self, transport_stream_id: u64) -> Option<DurableStreamHandle> {
        self.mappings
            .read()
            .expect("durable stream mapping lock poisoned")
            .get(&transport_stream_id)
            .map(|mapping| mapping.handle.clone())
    }

    fn mapping(&self, transport_stream_id: u64) -> Option<StreamSessionMappingRecord> {
        self.mappings
            .read()
            .expect("durable stream mapping lock poisoned")
            .get(&transport_stream_id)
            .cloned()
    }

    fn binding(&self, transport_stream_id: u64) -> Option<StreamBindingRecord> {
        self.bindings
            .read()
            .expect("durable stream binding lock poisoned")
            .get(&transport_stream_id)
            .cloned()
    }

    fn mapping_for_handle(
        &self,
        handle: &DurableStreamHandle,
        role: SessionStreamRole,
    ) -> Option<StreamSessionMappingRecord> {
        self.mappings
            .read()
            .expect("durable stream mapping lock poisoned")
            .iter()
            .find_map(|(_, mapping)| {
                (&mapping.handle == handle && mapping.role == role).then(|| mapping.clone())
            })
    }

    fn mapping_for_reference(
        &self,
        reference: &StreamRecordReference,
        role: SessionStreamRole,
    ) -> Option<StreamSessionMappingRecord> {
        let bindings = self
            .bindings
            .read()
            .expect("durable stream binding lock poisoned");
        let mappings = self
            .mappings
            .read()
            .expect("durable stream mapping lock poisoned");
        bindings.values().find_map(|binding| {
            (&binding.source == reference && binding.role == role)
                .then(|| mappings.get(&binding.transport_stream_id).cloned())
                .flatten()
        })
    }

    /// Returns the currently materialized transport mappings in transport-id order.
    pub fn materialized_mappings(&self) -> Vec<StreamSessionMappingRecord> {
        let mut mappings = self
            .mappings
            .read()
            .expect("durable stream mapping lock poisoned")
            .values()
            .cloned()
            .collect::<Vec<_>>();
        mappings.sort_by_key(|mapping| mapping.transport_stream_id);
        mappings
    }

    /// Adds a runtime binding without conflating independent readers of the same source.
    fn insert_binding_mapping(
        &self,
        binding: StreamBindingRecord,
        mapping: StreamSessionMappingRecord,
    ) -> Result<(), SessionError> {
        if binding.transport_stream_id != mapping.transport_stream_id
            || binding.role != mapping.role
        {
            return Err(SessionError::from(
                "durable stream binding differs from its runtime mapping".to_string(),
            ));
        }
        let transport_stream_id = mapping.transport_stream_id;
        let mut bindings = self
            .bindings
            .write()
            .expect("durable stream binding lock poisoned");
        let mut mappings = self
            .mappings
            .write()
            .expect("durable stream mapping lock poisoned");
        match (
            bindings.get(&transport_stream_id),
            mappings.get(&transport_stream_id),
        ) {
            (Some(existing_binding), Some(existing_mapping))
                if existing_binding == &binding && existing_mapping == &mapping =>
            {
                Ok(())
            }
            (Some(_), _) | (_, Some(_)) => Err(SessionError::from(format!(
                "transport stream id {transport_stream_id} is already mapped to another durable stream"
            ))),
            (None, None) => {
                self.next_transport_stream_id
                    .fetch_max(transport_stream_id.saturating_add(1), Ordering::AcqRel);
                bindings.insert(transport_stream_id, binding);
                mappings.insert(transport_stream_id, mapping);
                Ok(())
            }
        }
    }

    fn insert_mapping(&self, mapping: StreamSessionMappingRecord) -> Result<(), SessionError> {
        self.insert_binding_mapping(StreamBindingRecord::foreign(&mapping), mapping)
    }

    /// Commits and indexes a session record through the producer's owned write path.
    ///
    /// Every failure goes back to the session, not only a fence: a producer retired or poisoned
    /// under the session answers `RecoveryRequired`, and a failed publication `LiveBus`, neither
    /// of which says the record is invalid.
    async fn append_record(
        &self,
        context: Option<&StreamWriteContext>,
        record: StreamSessionRecord,
    ) -> Result<(), SessionError> {
        self.producer
            .append_session_record_attributed(context, self.entity_parent_start_index, record)
            .await
            .map_err(SessionError::from)
    }

    async fn try_append_record(
        &self,
        admission: &Arc<StreamWriteAdmission>,
        record: StreamSessionRecord,
    ) -> Result<(), SessionError> {
        let attribution = self.entity_parent_start_index;
        admission
            .submit(move |owner, context| async move {
                owner
                    .append_session_record_attributed(Some(&context), attribution, record)
                    .await
            })
            .await
            .map_err(SessionError::from)
    }

    async fn try_append_record_owned(
        &self,
        context: &StreamWriteContext,
        record: StreamSessionRecord,
    ) -> Result<(), SessionError> {
        self.producer
            .append_session_record_attributed(Some(context), self.entity_parent_start_index, record)
            .await
            .map_err(SessionError::from)
    }

    /// Returns the attempt identity recovered from the journal or durably records a fresh one.
    pub async fn caller_attempt_id(&self) -> Result<AttemptId, SessionError> {
        let session = self.clone();
        self.producer
            .run_admitted(None, 0, false, move |_, admission| async move {
                let _guard = session.session_lock.lock().await;
                let session_for_write = session.clone();
                admission
                    .submit(move |_, context| async move {
                        session_for_write.caller_attempt_id_owned(&context).await
                    })
                    .await
            })
            .await
    }

    async fn caller_attempt_id_owned(
        &self,
        context: &StreamWriteContext,
    ) -> Result<AttemptId, SessionError> {
        let metadata = self.current_control_metadata().await?;
        if let Some(attempt_id) = metadata.caller_attempt_id()? {
            return Ok(attempt_id);
        }
        drop(metadata);
        let attempt_id = AttemptId::fresh();
        self.try_append_record_owned(
            context,
            StreamSessionRecord::CallerAttempt(StreamCallerAttemptRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: self.session_reference.clone(),
                attempt_id,
            }),
        )
        .await?;
        self.commit_consumer_journal().await?;
        Ok(attempt_id)
    }

    async fn download_record(
        &self,
        record: OplogPayload<StreamSessionRecord>,
    ) -> Result<StreamSessionRecord, SessionError> {
        let record = self.oplog.download_payload(record).await?;
        if record.has_supported_format() {
            Ok(record)
        } else {
            Err(SessionError::from(
                "unsupported or malformed durable Stream Session record version".to_string(),
            ))
        }
    }

    /// Refreshes control metadata through the local oplog horizon, including buffered records.
    pub(crate) async fn current_control_metadata(
        &self,
    ) -> Result<tokio::sync::MutexGuard<'_, SessionControlMetadata>, SessionError> {
        let horizon = self.oplog.current_oplog_index().await;
        loop {
            if let Ok(metadata) = self.control_metadata.try_lock() {
                if metadata.covered_through() >= horizon {
                    metadata.ensure_valid()?;
                    return Ok(metadata);
                }
            } else {
                // A suspended store or select branch must not reserve this shared permit.
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                continue;
            }
            let streams = self.clone();
            self.producer
                .tasks()
                .spawn_metadata(async move { streams.refresh_control_metadata().await })
                .map_err(str::to_string)?
                .await
                .map_err(|error| format!("durable session metadata refresh failed: {error}"))??;
        }
    }

    async fn refresh_control_metadata(&self) -> Result<(), SessionError> {
        let mut metadata = self.control_metadata.lock().await;
        self.producer
            .refresh_control_metadata(&self.session_key, &mut metadata)
            .await
    }

    async fn session_record_at(
        &self,
        index: OplogIndex,
    ) -> Result<StreamSessionRecord, SessionError> {
        self.producer
            .read_session_record(index)
            .await
            .map_err(SessionError::from)
    }

    async fn append_mapping_once(
        &self,
        context: &StreamWriteContext,
        binding: StreamBindingRecord,
    ) -> Result<(), SessionError> {
        if self
            .current_control_metadata()
            .await?
            .has_explicit_mapping(&binding)
        {
            return Ok(());
        }
        self.producer
            .ensure_session_accepts_new_events(&self.session_key)
            .await
            .map_err(SessionError::from)?;
        self.try_append_record_owned(
            context,
            StreamSessionRecord::Mapping(StreamSessionMappingUpdateRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: self.session_reference.clone(),
                mapping: binding,
            }),
        )
        .await?;
        Ok(())
    }
}

struct ConsumerHistory {
    events: VecDeque<CommittedProducerStreamEvent>,
    nested_mappings: HashMap<StreamOffset, Vec<StreamSessionMappingRecord>>,
    observations: HashMap<u64, ConsumerObservations>,
    after: Option<golem_common::model::durable_stream::StreamOffset>,
    terminal: bool,
}

struct ConsumerObservations {
    original_index: OplogIndex,
    boundaries: Vec<(OplogIndex, StreamConsumerObservation)>,
}

/// Owns source reads, journal replay, and read ordinals independently of Wasmtime polling.
/// Session topology and cancellation remain on its binding-local `StreamSession`.
pub struct DurableInputEndpoint {
    reader: Option<DurableStreamReader>,
    journal: VecDeque<CommittedProducerStreamEvent>,
    nested_mappings: HashMap<StreamOffset, Vec<StreamSessionMappingRecord>>,
    observations: HashMap<u64, ConsumerObservations>,
    source_after: Option<golem_common::model::durable_stream::StreamOffset>,
    source_terminal: bool,
    streams: StreamSession,
    transport_stream_id: u64,
    handle: DurableStreamHandle,
    consumer_read_ordinal: u64,
    role: SessionStreamRole,
    reader_id: DurableReaderIdentity,
}

type DurableReaderIdentity = Shared<BoxFuture<'static, Result<LocalStreamReaderId, String>>>;

/// Foreign source and attachment state for a forwarded durable input.
pub struct ForwardedDurableInput {
    pub handle: DurableStreamHandle,
    origin: StreamSession,
    reader_id: DurableReaderIdentity,
}

enum ForwardedDurableInputReference {
    Forwarded(DurableStreamHandle),
    Endpoint(DurableStreamHandle),
}

enum DurableStreamReader {
    Owned {
        source_wait_can_suspend: bool,
        reader: Box<DurableCatchUpReader>,
        source: Arc<DurableStreamStore>,
        handle: Box<DurableStreamHandle>,
        foreign_binding: bool,
        next_journal_lag_sample: Instant,
    },
    Attached(Box<AttachedDurableCatchUpReader>),
}

struct AttachedDurableCatchUpReader {
    source: Arc<dyn AttachedStreamSegmentSource>,
    attachment: StreamAttachmentKey,
    handle: DurableStreamHandle,
    consumer_producer: Option<(Arc<DurableStreamStore>, LocalStreamReaderId)>,
    after: Option<golem_common::model::durable_stream::StreamOffset>,
    buffered: VecDeque<CommittedProducerStreamEvent>,
    terminal: bool,
    next_journal_lag_sample: Instant,
}

struct DurableInputRead {
    reader: Option<DurableStreamReader>,
    event: Option<CommittedProducerStreamEvent>,
    endpoints: HashMap<u64, DurableInputEndpoint>,
    guest_bytes: Option<bytes::Bytes>,
    #[cfg(test)]
    journaled: bool,
    queued_events: VecDeque<CommittedProducerStreamEvent>,
    observation: Option<PendingConsumerObservation>,
}

struct PendingConsumerObservation {
    record: StreamSessionRecord,
    recorded: Option<ConsumerObservations>,
    introduced: Option<oneshot::Sender<Result<OplogIndex, String>>>,
}

#[derive(Debug)]
enum DurableReceiveError {
    Session(SessionError),
    Interrupt(InterruptKind),
}

type DurableReceiveFuture = BoxFuture<'static, Result<DurableInputRead, DurableReceiveError>>;

struct ReceiveGuard {
    _live_call: LiveCallPermit,
    interrupt: Option<crate::durable_host::stream_transport::FrontendInterrupt>,
}

struct DurableReadWait {
    runtime: Arc<RuntimeStore>,
    activity: Arc<OnceLock<RuntimeActivityId>>,
    grace: Duration,
    recheck: Duration,
    interrupt: BoxFuture<'static, InterruptKind>,
}

/// Adapts a durable input endpoint to Wasmtime polling, values, and guest-drop cleanup.
pub struct DurableInputProducer {
    input: DurableInputEndpoint,
    /// Guest acknowledgements lag the endpoint's host-take ordinal while delivery is pending.
    guest_read_ordinal: Arc<AtomicU64>,
    pending: Option<DurableReceiveFuture>,
    pending_activity: Option<Arc<OnceLock<RuntimeActivityId>>>,
    live_admission: Option<oneshot::Receiver<Result<(), WorkerExecutorError>>>,
    pending_drop: Option<BoxFuture<'static, Result<(), SessionError>>>,
    pending_guest_value: Option<DurableInputItem>,
    guest_delivery: Option<oneshot::Receiver<Result<StreamReadObserver, WorkerExecutorError>>>,
    pending_observation: Option<PendingConsumerObservation>,
    finished: bool,
    dropping: bool,
    drop_event_sink: Option<mpsc::UnboundedSender<DropEvent>>,
    runtime_teardown: Arc<dyn Fn() -> bool + Send + Sync>,
    /// Receives the reason of a cancel by the input's producer before the end is returned.
    producer_cancel: Option<Arc<OnceLock<StreamCancelReason>>>,
    /// See [`Self::with_pending_reads_past_a_completed_record`].
    pending_past_a_completed_record: bool,
}

struct PrepareInputObservation {
    observation: PendingConsumerObservation,
    streams: StreamSession,
    replay: ReplayState,
    guest_order: Option<super::replay_state::GuestReadOrder>,
    runtime: Option<Arc<crate::worker::suspension::RuntimeStore>>,
    activity: TailActivity,
    result: oneshot::Sender<Result<StreamReadObserver, WorkerExecutorError>>,
    runtime_teardown: Arc<dyn Fn() -> bool + Send + Sync>,
    cleanup_sink: mpsc::UnboundedSender<DropEvent>,
    trap_context: DurableCallTrapContext,
    live_call_permit: LiveCallPermit,
}

struct DurableInputLiveAdmission<Ctx> {
    replay_state: ReplayState,
    activity: TailActivity,
    boundary: SerializableToolAdmissionRefusal,
    result: oneshot::Sender<Result<(), WorkerExecutorError>>,
    _ctx: PhantomData<fn() -> Ctx>,
}

#[derive(Debug)]
pub(crate) enum DurableInputEvent {
    Item(SchemaValue),
    End,
    Cancelled,
}

pub(crate) struct DurableInputReceiveAdmission {
    opens_source: bool,
    ordinal: u64,
    boundary: SerializableToolAdmissionRefusal,
    result: oneshot::Sender<Result<ReceiveGuard, WorkerExecutorError>>,
}

/// Deferred cleanup returned when a guest drops an unread durable input.
#[derive(Clone)]
pub struct DroppedDurableInput {
    streams: StreamSession,
    transport_stream_id: u64,
    consumer_read_ordinal: u64,
    role: StreamCancelRole,
}

struct DurableInputDropTask {
    cancellation: DroppedDurableInput,
    result: oneshot::Sender<Result<(), SessionError>>,
}

enum DurableInputItem {
    Value(SchemaValue),
    Bytes(bytes::Bytes),
    Terminal(StreamResult),
}

/// Adapts the same durable input journal to native tools' byte-stream ABI.
pub struct DurableByteInputProducer(pub DurableInputProducer);

/// What the stdin of a tool body gets for one read of its durable input.
#[derive(Debug)]
enum ToolStdinDelivery {
    Chunk(Vec<u8>),
    /// The one item that says that the stdin was cancelled. The stream ends after it.
    Cancelled,
    End(StreamResult),
}

/// Adapts the durable stdin of a tool call that a client started to the stdin of a component
/// tool body.
///
/// The body reads the durable input itself. When its guest reads, each chunk and the terminal
/// is a guest observation of the body, recorded where the guest takes it, so a replay hands it
/// over at the same place among the body's other recorded calls. When the host reads, for a
/// tool that the body forwards its stdin to, the reads are host records.
///
/// The records stay in the session of the call and keep its attribution, because one session
/// has one attribution. The observation names the body as its parent, and that is what a
/// replay reads.
///
/// A guest read asks for up to [`MAX_PACKED_U8_STREAM_ITEM_SIZE`] bytes. The size is fixed:
/// a replay that asked for less would cut a recorded chunk in another place.
///
/// Dropping the reader cancels nothing. The end of the session cancels what nobody read, so a
/// later attempt of the same call finds the stdin as the client left it.
pub struct DurableToolStdinProducer {
    input: DurableInputProducer,
    producer_cancel: Arc<OnceLock<StreamCancelReason>>,
    /// The `cancelled` item was handed over. The next read ends the stream.
    ended: bool,
}

#[cfg(test)]
mod tests;
