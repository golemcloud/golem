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

use crate::durable_host::concurrent::DropEvent;
use crate::durable_host::durability::{ClassifiedHostError, HostFailureKind};
use crate::durable_host::durable_stream::{
    AttachedStreamSegmentSource, CommittedProducerStreamEvent, CommittedProducerStreamEventPayload,
    ConsumerAttachmentStatus, DurableCatchUpReader, DurableStreamStore, NestedStreamWrite,
    ProducerOutputRegistration, ProducerOutputSource, ProducerRegistrationRequest,
    RoutedAttachedStreamSegmentSource, RoutedStreamAttachmentControl, SessionControlMetadata,
    StreamAttachmentConsumerProbe, StreamAttachmentControl, StreamSegmentSource, StreamStoreError,
    StreamWriteAdmission, StreamWriteContext,
};
use crate::durable_host::schema_value_stream::StoreValueResolver;
use crate::durable_host::stream_bus::{LiveStreamEventPayload, LiveStreamReceiveError};
use crate::durable_host::stream_session::{
    decode_recursive_stream_value, decode_recursive_stream_value_with_schema,
    encode_recursive_stream_value_with_schema, preflight_proto_recursive_stream_value,
    preflight_recursive_stream_value, remap_recursive_stream_references,
};
use crate::durable_host::stream_transport::{LiveStreamEndpoint, SourceLifecycle};
use crate::durable_host::suspendable_wait::SuspendableWaitRegistration;
use crate::services::oplog::{Oplog, OplogOps};
use crate::services::rpc::Rpc;
use crate::workerctx::WorkerCtx;
use futures::future::try_join_all;
use golem_api_grpc::proto::golem::schema::SchemaValue as ProtoSchemaValue;
use golem_api_grpc::proto::golem::worker::{
    DurableStreamHandle as ProtoDurableStreamHandle, DurableStreamMapping,
    InputStreamHighWater as ProtoInputStreamHighWater, InvocationResponse, OutputStreamEnd,
    OutputStreamError, OutputStreamItem, StreamCancel, StreamInvocationIdentity, StreamMappingRole,
    invocation_response,
};
use golem_common::base_model::durable_stream::{
    AttachmentId, AttemptId, DURABLE_STREAM_FORMAT_VERSION, DurableStreamHandle,
    InputStreamHighWater, MAX_NEW_STREAM_HANDLES_PER_VALUE, MAX_PACKED_U8_STREAM_ITEM_SIZE,
    SessionStreamRole, StreamAttachmentKey, StreamCallerAttemptRecord, StreamCancelReason,
    StreamCancelRole, StreamConsumerCancelAppliedRecord, StreamConsumerCancelIntentRecord,
    StreamConsumerItemValueRecord, StreamConsumerTerminal, StreamConsumerTerminalRecord,
    StreamEndResult, StreamInvocationId, StreamItemsPayload, StreamRegistrationCoordinate,
    StreamResumeOperation, StreamRootKind, StreamSessionCancelRequestedRecord,
    StreamSessionDetachedRecord, StreamSessionInvocationResultRecord, StreamSessionKey,
    StreamSessionMapping, StreamSessionMappingRecord, StreamSessionMappingUpdateRecord,
    StreamSessionRecord, StreamSessionResumeAttemptRecord, StreamSlotTombstonedRecord,
    StreamSourceKind, StreamTopologyActivatedRecord, StreamTopologyPreparedRecord,
    StreamValuePathStep,
};
use golem_common::base_model::oplog::OplogEntry;
use golem_common::model::Timestamp;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::oplog::payload::OplogPayload;
use golem_schema::schema::wit::{encode_value_with_streams, wire};
use golem_schema::schema::{SchemaFingerprintV1, SchemaGraph, SchemaType, schema_fingerprint_v1};
use golem_schema::schema::{SchemaValue, SchemaValueStream};
use golem_service_base::model::auth::AuthCtx;
use prost::Message;
use std::any::{Any, TypeId};
use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, mpsc};
use wasmtime::StoreContextMut;
use wasmtime::component::{Destination, StreamProducer, StreamResult};

const PACKED_U8_OUTPUT_FLUSH_DELAY: Duration = Duration::from_millis(50);

#[async_trait::async_trait]
/// Commits consumer journal entries and queries their durable completion boundary.
pub(crate) trait DurableStreamConsumerJournal: Send + Sync {
    /// Makes previously appended consumer observations recoverable.
    async fn commit(&self) -> Result<(), String>;
    /// Returns the committed session-finished index, if present.
    async fn committed_finished_index(
        &self,
        session: &StreamSessionKey,
    ) -> Result<Option<OplogIndex>, String>;
}

#[derive(Clone)]
/// Runs one durable Stream Session's disposable mappings and transport collaborators.
/// Persisted stream records and indexes remain owned by `DurableStreamStore`.
pub(crate) struct StreamSession {
    pub(crate) producer: Arc<DurableStreamStore>,
    pub(crate) oplog: Arc<dyn Oplog>,
    pub(crate) session_key: StreamSessionKey,
    consumer_invocation: StreamInvocationId,
    mappings: Arc<RwLock<HashMap<u64, StreamSessionMappingRecord>>>,
    input_schema: Option<Arc<DurableInputSchema>>,
    rpc: Option<Arc<dyn Rpc>>,
    consumer_journal: Option<Arc<dyn DurableStreamConsumerJournal>>,
    auth_ctx: Option<AuthCtx>,
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

impl Drop for OutputDrainRegistration {
    fn drop(&mut self) {
        self.producer
            .unregister_source_cancellation(self.stream_id, self.registration_id);
        self.lifecycle.finish();
    }
}

struct PendingOwnedStreamDrain {
    handle: DurableStreamHandle,
    endpoint: LiveStreamEndpoint,
    element_type: SchemaType,
    role: SessionStreamRole,
}

/// Encodes a fingerprint-bound session mapping for transport.
pub(crate) fn durable_stream_mapping_to_proto(
    mapping: &StreamSessionMappingRecord,
    high_water: Option<&InputStreamHighWater>,
) -> DurableStreamMapping {
    DurableStreamMapping {
        transport_stream_id: mapping.transport_stream_id,
        handle: Some(ProtoDurableStreamHandle {
            format_version: u32::from(mapping.handle.format_version),
            stream_id: Some(mapping.handle.stream_id.0.into()),
            producer_environment_id: Some(mapping.handle.producer_environment_id.into()),
            producer: Some(mapping.handle.producer.clone().into()),
            expected_producer_fingerprint: Some(
                mapping.handle.expected_producer_fingerprint.0.into(),
            ),
            source_invocation: Some(StreamInvocationIdentity {
                callee_environment_id: Some(
                    mapping
                        .handle
                        .source_invocation
                        .callee_environment_id
                        .into(),
                ),
                callee: Some(mapping.handle.source_invocation.callee.clone().into()),
                callee_fingerprint: Some(
                    mapping.handle.source_invocation.callee_fingerprint.0.into(),
                ),
                idempotency_key: Some(
                    mapping
                        .handle
                        .source_invocation
                        .idempotency_key
                        .clone()
                        .into(),
                ),
            }),
            component_revision: Some(mapping.handle.component_revision.get()),
            element_schema_fingerprint: mapping.handle.element_schema_fingerprint.0.to_vec(),
        }),
        high_water: high_water.map(|high_water| ProtoInputStreamHighWater {
            highest_contiguous_sequence: high_water.highest_contiguous_sequence,
            resulting_offset: high_water.resulting_offset.as_bytes().to_vec(),
            terminal: high_water.terminal,
        }),
        role: match mapping.role {
            SessionStreamRole::Input => StreamMappingRole::Input as i32,
            SessionStreamRole::Output => StreamMappingRole::Output as i32,
        },
    }
}

/// Decodes and validates the complete durable identity carried by a mapping.
pub(crate) fn durable_stream_mapping_from_proto(
    mapping: DurableStreamMapping,
) -> Result<StreamSessionMappingRecord, String> {
    let handle = mapping
        .handle
        .ok_or_else(|| "durable stream mapping has no handle".to_string())?;
    if handle.format_version != u32::from(DURABLE_STREAM_FORMAT_VERSION) {
        return Err("unsupported durable stream handle format version".to_string());
    }
    let source = handle
        .source_invocation
        .ok_or_else(|| "durable stream handle has no source invocation".to_string())?;
    let element_schema_fingerprint: [u8; 32] = handle
        .element_schema_fingerprint
        .try_into()
        .map_err(|_| "durable stream schema fingerprint must contain 32 bytes".to_string())?;
    let role =
        match StreamMappingRole::try_from(mapping.role).unwrap_or(StreamMappingRole::Unspecified) {
            StreamMappingRole::Input => SessionStreamRole::Input,
            StreamMappingRole::Output => SessionStreamRole::Output,
            StreamMappingRole::Unspecified => {
                return Err("durable stream mapping has no role".to_string());
            }
        };
    Ok(StreamSessionMappingRecord {
        transport_stream_id: mapping.transport_stream_id,
        handle: DurableStreamHandle {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            stream_id: golem_common::model::durable_stream::StreamId(
                handle
                    .stream_id
                    .ok_or_else(|| "durable stream handle has no stream ID".to_string())?
                    .into(),
            ),
            producer_environment_id: handle
                .producer_environment_id
                .ok_or_else(|| "durable stream handle has no producer environment".to_string())?
                .try_into()?,
            producer: handle
                .producer
                .ok_or_else(|| "durable stream handle has no producer".to_string())?
                .try_into()?,
            expected_producer_fingerprint: golem_common::model::AgentFingerprint(
                handle
                    .expected_producer_fingerprint
                    .ok_or_else(|| "durable stream handle has no producer fingerprint".to_string())?
                    .into(),
            ),
            source_invocation: StreamInvocationId {
                callee_environment_id: source
                    .callee_environment_id
                    .ok_or_else(|| "durable stream source has no callee environment".to_string())?
                    .try_into()?,
                callee: source
                    .callee
                    .ok_or_else(|| "durable stream source has no callee".to_string())?
                    .try_into()?,
                callee_fingerprint: golem_common::model::AgentFingerprint(
                    source
                        .callee_fingerprint
                        .ok_or_else(|| {
                            "durable stream source has no callee fingerprint".to_string()
                        })?
                        .into(),
                ),
                idempotency_key: source
                    .idempotency_key
                    .ok_or_else(|| "durable stream source has no idempotency key".to_string())?
                    .into(),
            },
            component_revision: golem_common::model::component::ComponentRevision::new(
                handle
                    .component_revision
                    .ok_or_else(|| "durable stream handle has no component revision".to_string())?,
            )
            .map_err(|error| error.to_string())?,
            element_schema_fingerprint: SchemaFingerprintV1(element_schema_fingerprint),
        },
        role,
    })
}

fn stream_cancel_reason_to_proto(
    reason: StreamCancelReason,
) -> golem_api_grpc::proto::golem::worker::StreamCancelReason {
    match reason {
        StreamCancelReason::Cancelled => {
            golem_api_grpc::proto::golem::worker::StreamCancelReason::Cancelled
        }
        StreamCancelReason::GuestDrop => {
            golem_api_grpc::proto::golem::worker::StreamCancelReason::ConsumerDrop
        }
        StreamCancelReason::Protocol => {
            golem_api_grpc::proto::golem::worker::StreamCancelReason::Protocol
        }
        StreamCancelReason::InvocationFailed => {
            golem_api_grpc::proto::golem::worker::StreamCancelReason::InvocationFailed
        }
        StreamCancelReason::SourceUnavailable => {
            golem_api_grpc::proto::golem::worker::StreamCancelReason::SourceUnavailable
        }
        StreamCancelReason::ProducerDeleting => {
            golem_api_grpc::proto::golem::worker::StreamCancelReason::ProducerDeleting
        }
    }
}

impl StreamSession {
    /// Creates session runtime state from its durable identity and known mappings.
    pub(crate) fn new(
        producer: Arc<DurableStreamStore>,
        oplog: Arc<dyn Oplog>,
        session_key: StreamSessionKey,
        mappings: impl IntoIterator<Item = StreamSessionMappingRecord>,
    ) -> Self {
        let session_lock = producer.session_lock(&session_key);
        let mappings = mappings
            .into_iter()
            .map(|mapping| (mapping.transport_stream_id, mapping))
            .collect::<HashMap<_, _>>();
        let next_transport_stream_id = mappings
            .keys()
            .copied()
            .max()
            .map(|id| id.saturating_add(1))
            .unwrap_or_default();
        Self {
            producer,
            oplog,
            consumer_invocation: session_key.clone(),
            session_key,
            mappings: Arc::new(RwLock::new(mappings)),
            input_schema: None,
            rpc: None,
            consumer_journal: None,
            auth_ctx: None,
            require_root_attachment_before_production: false,
            next_transport_stream_id: Arc::new(AtomicU64::new(next_transport_stream_id)),
            session_lock,
            attachment_epoch: 1,
            attachment_attempt_id: None,
            entity_parent_start_index: None,
            recovered_mappings_through: Arc::new(Mutex::new(OplogIndex::NONE)),
            control_metadata: Arc::new(Mutex::new(SessionControlMetadata::default())),
            response_lease: None,
        }
    }

    /// Retains an ephemeral response only for the lifetime of this session runtime.
    pub(crate) fn with_response_lease(
        mut self,
        lease: Option<Arc<crate::worker::EphemeralResponseLease>>,
    ) -> Self {
        self.response_lease = lease;
        self
    }

    /// Returns the resident ephemeral-response lease, if one is held.
    pub(crate) fn response_lease(&self) -> Option<Arc<crate::worker::EphemeralResponseLease>> {
        self.response_lease.clone()
    }

    /// Binds this runtime to one attachment epoch and attempt.
    pub(crate) fn with_attachment(mut self, epoch: u64, attempt_id: AttemptId) -> Self {
        self.recovered_mappings_through = Arc::new(Mutex::new(OplogIndex::NONE));
        self.attachment_epoch = epoch;
        self.attachment_attempt_id = Some(attempt_id);
        self
    }

    /// Sets the invocation identity attributed to consumer journal records.
    pub(crate) fn with_consumer_invocation(
        mut self,
        consumer_invocation: StreamInvocationId,
    ) -> Self {
        self.recovered_mappings_through = Arc::new(Mutex::new(OplogIndex::NONE));
        self.consumer_invocation = consumer_invocation;
        self
    }

    /// Attributes session records to an entity call in the owner's oplog.
    pub(crate) fn with_entity_parent_start_index(
        mut self,
        entity_parent_start_index: Option<OplogIndex>,
    ) -> Self {
        self.entity_parent_start_index = entity_parent_start_index;
        self
    }

    /// Enables routed producer attachment and segment operations.
    pub(crate) fn with_rpc(mut self, rpc: Arc<dyn Rpc>) -> Self {
        self.rpc = Some(rpc);
        self
    }

    /// Installs the journal whose commit makes guest observations durable.
    pub(crate) fn with_consumer_journal(
        mut self,
        consumer_journal: Arc<dyn DurableStreamConsumerJournal>,
    ) -> Self {
        self.consumer_journal = Some(consumer_journal);
        self
    }

    /// Commits appended consumer observations before they are returned to the guest.
    pub(crate) async fn commit_consumer_journal(&self) -> Result<(), String> {
        self.consumer_journal
            .as_ref()
            .ok_or_else(|| "durable stream consumer journal commit is unavailable".to_string())?
            .commit()
            .await
    }

    /// Preserves the original consumer authorization for routed source operations.
    pub(crate) fn with_auth_ctx(mut self, auth_ctx: AuthCtx) -> Self {
        self.auth_ctx = Some(auth_ctx);
        self
    }

    /// Requires durable root attachment activation before open output production.
    pub(crate) fn require_root_attachment_before_production(mut self) -> Self {
        self.require_root_attachment_before_production = true;
        self
    }

    fn allocate_transport_stream_id(&self) -> Result<u64, String> {
        self.next_transport_stream_id
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(1)
            })
            .map_err(|_| "durable transport stream id overflow".to_string())
    }

    /// Pins input element schemas and component revision for this durable session.
    pub(crate) fn with_input_schema(
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
    pub(crate) fn handle(&self, transport_stream_id: u64) -> Option<DurableStreamHandle> {
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

    /// Validates frame epoch, attachment attempt, durable stream ID, and role.
    pub(crate) async fn validate_frame(
        &self,
        transport_stream_id: u64,
        durable_stream_id: Option<golem_api_grpc::proto::golem::common::Uuid>,
        epoch: u64,
        expected_role: SessionStreamRole,
    ) -> Result<DurableStreamHandle, String> {
        let (current_epoch, current_attempt_id, attached) =
            self.authoritative_attachment_state().await?;
        if epoch != current_epoch || self.attachment_epoch != current_epoch {
            return Err(
                if epoch < current_epoch || self.attachment_epoch < current_epoch {
                    "StaleEpoch: durable stream frame uses a fenced attachment epoch".to_string()
                } else {
                    "InvalidEpoch: durable stream frame uses a future attachment epoch".to_string()
                },
            );
        }
        if !attached || self.attachment_attempt_id != Some(current_attempt_id) {
            return Err("StaleEpoch: durable stream frame uses a detached attachment".to_string());
        }
        let durable_stream_id: uuid::Uuid = durable_stream_id
            .ok_or_else(|| "durable stream frame has no durable stream ID".to_string())?
            .into();
        let mappings = self
            .mappings
            .read()
            .expect("durable stream mapping lock poisoned");
        let mapping = mappings
            .get(&transport_stream_id)
            .ok_or_else(|| format!("unknown durable transport stream ID {transport_stream_id}"))?;
        if mapping.handle.stream_id.0 != durable_stream_id || mapping.role != expected_role {
            return Err(
                "transport stream mapping does not match the durable stream ID and role"
                    .to_string(),
            );
        }
        Ok(mapping.handle.clone())
    }

    /// Returns the attachment epoch represented by this runtime.
    pub(crate) fn attachment_epoch(&self) -> u64 {
        self.attachment_epoch
    }

    /// Rejects this runtime if durable authority has detached or advanced its epoch.
    pub(crate) async fn ensure_current_attachment(&self) -> Result<(), String> {
        let (epoch, attempt_id, attached) = self.authoritative_attachment_state().await?;
        if epoch != self.attachment_epoch
            || self.attachment_attempt_id != Some(attempt_id)
            || !attached
        {
            return Err("StaleEpoch: durable attachment has been fenced".to_string());
        }
        Ok(())
    }

    /// Waits until durable attachment authority fences this runtime.
    pub(crate) async fn wait_for_attachment_revocation(&self) -> Result<(), String> {
        loop {
            let changed = self.producer.session_records_changed().notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            match self.ensure_current_attachment().await {
                Ok(()) => changed.await,
                Err(error) if error.starts_with("StaleEpoch:") => return Ok(()),
                Err(error) => return Err(error),
            }
        }
    }

    /// Reads attachment epoch, attempt, and attached state from durable oplog metadata.
    pub(crate) async fn authoritative_attachment_state(
        &self,
    ) -> Result<(u64, AttemptId, bool), String> {
        let raw = self
            .oplog
            .raw_durable_stream_session_status(&self.session_key)
            .await;
        let status = raw
            .status?
            .ok_or_else(|| "durable session has no attachment authority".to_string())?;
        if let Some(error) = status.lifecycle_error {
            return Err(error);
        }
        match (
            status.attachment_epoch,
            status.attachment_attempt_id,
            status.attachment_attached,
        ) {
            (Some(epoch), Some(attempt), Some(attached)) => Ok((epoch, attempt, attached)),
            _ => Err("durable session has no attachment authority".to_string()),
        }
    }

    /// Durably detaches the current transport without cancelling producer streams.
    pub(crate) async fn detach_current(&self) -> Result<bool, String> {
        let session = self.clone();
        self.producer
            .run_admitted(None, 0, true, move |_, admission| async move {
                let _session_guard = session.session_lock.lock().await;
                let session_for_write = session.clone();
                admission
                    .submit(move |_, context| async move {
                        session_for_write.detach_current_owned(&context).await
                    })
                    .await
            })
            .await
    }

    async fn detach_current_owned(&self, context: &StreamWriteContext) -> Result<bool, String> {
        let (epoch, attempt_id, attached) = self.authoritative_attachment_state().await?;
        if !attached
            || epoch != self.attachment_epoch
            || self.attachment_attempt_id != Some(attempt_id)
        {
            return Ok(false);
        }
        self.append_record(
            Some(context),
            StreamSessionRecord::Detached(StreamSessionDetachedRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: self.session_key.clone(),
                attachment_id: golem_common::model::durable_stream::AttachmentId::primary(
                    self.session_key.callee_environment_id,
                    &self.session_key.callee,
                    &self.session_key.idempotency_key,
                )
                .map_err(|error| error.to_string())?,
                owner_attempt_id: attempt_id,
                epoch,
            }),
        )
        .await;
        self.commit_consumer_journal().await?;
        Ok(true)
    }

    #[tracing::instrument(
        name = "durable_stream.resume_attempt",
        skip_all,
        fields(
            attachment_id = %record.attempt.attachment_id.0,
            attempt_id = %record.attempt.attempt_id.0,
            operation = ?record.attempt.operation,
            expected_epoch = record.attempt.expected_epoch,
            accepted_epoch = record.accepted_epoch,
        )
    )]
    /// Commits a resume or takeover attempt before the new epoch is used for frames.
    pub(crate) async fn commit_resume_attempt(
        &self,
        record: StreamSessionResumeAttemptRecord,
    ) -> Result<(), String> {
        let memory = golem_common::serialization::serialize(&record)?.len();
        let session = self.clone();
        self.producer
            .run_admitted(None, memory, true, move |_, admission| async move {
                let _session_guard = session.session_lock.lock().await;
                let session_for_write = session.clone();
                admission
                    .submit(move |_, context| async move {
                        session_for_write
                            .commit_resume_attempt_owned(&context, record)
                            .await
                    })
                    .await
            })
            .await
    }

    async fn commit_resume_attempt_owned(
        &self,
        context: &StreamWriteContext,
        record: StreamSessionResumeAttemptRecord,
    ) -> Result<(), String> {
        let (current_epoch, _, attached) = self.authoritative_attachment_state().await?;
        if record.attempt.expected_epoch < current_epoch {
            return Err(format!(
                "StaleEpoch: current attachment epoch is {current_epoch}"
            ));
        }
        if record.attempt.expected_epoch > current_epoch {
            return Err(format!(
                "InvalidEpoch: current attachment epoch is {current_epoch}"
            ));
        }
        match (record.attempt.operation, attached) {
            (StreamResumeOperation::Resume, false) | (StreamResumeOperation::Takeover, true) => {}
            _ => {
                return Err(
                    "InvalidAttachmentState: resume requires Detached and takeover requires Attached"
                        .to_string(),
                );
            }
        }
        let result = self
            .producer
            .append_session_record_attributed(
                Some(context),
                self.entity_parent_start_index,
                StreamSessionRecord::ResumeAttempt(record),
            )
            .await
            .map_err(|error| error.to_string());
        if result.is_ok() {
            tracing::debug!("Durable Stream Session resume attempt committed");
        }
        result
    }

    /// Adds a runtime mapping only when its durable stream identity and role agree.
    pub(crate) fn insert_mapping(&self, mapping: StreamSessionMappingRecord) -> Result<(), String> {
        let StreamSessionMappingRecord {
            transport_stream_id,
            ref handle,
            role,
        } = mapping;
        let mut mappings = self
            .mappings
            .write()
            .expect("durable stream mapping lock poisoned");
        if let Some((existing_transport_stream_id, _)) = mappings
            .iter()
            .find(|(_, existing)| &existing.handle == handle && existing.role == role)
            && *existing_transport_stream_id != transport_stream_id
        {
            return Err(format!(
                "durable stream is already mapped to transport stream id {existing_transport_stream_id}"
            ));
        }
        match mappings.get(&transport_stream_id) {
            Some(existing) if existing == &mapping => Ok(()),
            Some(_) => Err(format!(
                "transport stream id {transport_stream_id} is already mapped to another durable stream"
            )),
            None => {
                self.next_transport_stream_id
                    .fetch_max(transport_stream_id.saturating_add(1), Ordering::AcqRel);
                mappings.insert(transport_stream_id, mapping);
                Ok(())
            }
        }
    }

    /// Commits and indexes a session record through the producer's owned write path.
    pub(crate) async fn append_record(
        &self,
        context: Option<&StreamWriteContext>,
        record: StreamSessionRecord,
    ) {
        self.producer
            .append_session_record_attributed(context, self.entity_parent_start_index, record)
            .await
            .expect("internally generated durable session record is valid");
    }

    async fn try_append_record(
        &self,
        admission: &Arc<StreamWriteAdmission>,
        record: StreamSessionRecord,
    ) -> Result<(), String> {
        let attribution = self.entity_parent_start_index;
        admission
            .submit(move |owner, context| async move {
                owner
                    .append_session_record_attributed(Some(&context), attribution, record)
                    .await
            })
            .await
            .map_err(|error| error.to_string())
    }

    /// Returns the attempt identity recovered from the journal or durably records a fresh one.
    pub(crate) async fn caller_attempt_id(&self) -> Result<AttemptId, String> {
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
    ) -> Result<AttemptId, String> {
        let metadata = self.current_control_metadata().await?;
        if metadata.caller_attempt_conflict {
            return Err(
                "conflicting caller attempt IDs are persisted for the Stream Session".into(),
            );
        }
        if let Some(attempt_id) = metadata.caller_attempt {
            return Ok(attempt_id);
        }
        drop(metadata);
        let attempt_id = AttemptId::fresh();
        self.append_record(
            Some(context),
            StreamSessionRecord::CallerAttempt(StreamCallerAttemptRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: self.session_key.clone(),
                attempt_id,
            }),
        )
        .await;
        self.commit_consumer_journal().await?;
        Ok(attempt_id)
    }

    async fn download_record(
        &self,
        record: OplogPayload<StreamSessionRecord>,
    ) -> Result<StreamSessionRecord, String> {
        let record = self.oplog.download_payload(record).await?;
        if record.has_supported_format() {
            Ok(record)
        } else {
            Err("unsupported or malformed durable Stream Session record version".to_string())
        }
    }

    /// Refreshes control metadata through the local oplog horizon, including buffered records.
    pub(crate) async fn current_control_metadata(
        &self,
    ) -> Result<tokio::sync::MutexGuard<'_, SessionControlMetadata>, String> {
        let horizon = self.oplog.current_oplog_index().await;
        loop {
            if let Ok(metadata) = self.control_metadata.try_lock() {
                if metadata.covered_through >= horizon {
                    if metadata.malformed_record {
                        return Err(
                            "unsupported or malformed durable Stream Session record version".into(),
                        );
                    }
                    return Ok(metadata);
                }
            } else {
                // A suspended store or select branch must not reserve this shared permit.
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                continue;
            }
            let streams = self.clone();
            tokio::spawn(async move { streams.refresh_control_metadata().await })
                .await
                .map_err(|error| format!("durable session metadata refresh failed: {error}"))??;
        }
    }

    async fn refresh_control_metadata(&self) -> Result<(), String> {
        let mut metadata = self.control_metadata.lock().await;
        if !metadata.covered_through.is_defined()
            && let Some(persisted) = self
                .producer
                .persisted_control_metadata(&self.session_key)
                .await?
        {
            *metadata = persisted;
        }
        if metadata.malformed_record {
            return Err("unsupported or malformed durable Stream Session record version".into());
        }
        // This is deliberately the raw horizon, not the last published AgentStatusRecord:
        // local control records remain visible before and after the append buffer is committed.
        let horizon = self.oplog.current_oplog_index().await;
        while metadata.covered_through < horizon {
            let count = (horizon.as_u64() - metadata.covered_through.as_u64()).min(1024);
            for (index, entry) in self
                .oplog
                .read_exact(metadata.covered_through.next(), count)
                .await
            {
                if let OplogEntry::StreamSession { record, .. } = entry {
                    let record = self.download_record(record).await?;
                    metadata.apply(index, &self.session_key, &record);
                } else {
                    metadata.covered_through = index;
                }
            }
        }
        Ok(())
    }

    async fn session_record_at(&self, index: OplogIndex) -> Result<StreamSessionRecord, String> {
        let OplogEntry::StreamSession { record, .. } = self.oplog.read(index).await else {
            return Err("durable session metadata points at a non-session record".into());
        };
        self.download_record(record).await
    }

    async fn append_mapping_once(
        &self,
        context: &StreamWriteContext,
        mapping: StreamSessionMappingRecord,
    ) -> Result<(), String> {
        if self
            .current_control_metadata()
            .await?
            .explicit_mappings
            .contains(&(
                mapping.transport_stream_id,
                mapping.handle.clone(),
                mapping.role,
            ))
        {
            return Ok(());
        }
        self.producer
            .ensure_session_accepts_new_events(&self.session_key)
            .await
            .map_err(|error| error.to_string())?;
        self.append_record(
            Some(context),
            StreamSessionRecord::Mapping(StreamSessionMappingUpdateRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: self.session_key.clone(),
                mapping,
            }),
        )
        .await;
        Ok(())
    }

    /// Activates a prepared forwarded topology after validating producer and consumer identities.
    pub(crate) async fn activate_forwarded_mapping(
        &self,
        attachment: StreamAttachmentKey,
        mapping: StreamSessionMappingRecord,
        producer_control: Arc<dyn StreamAttachmentControl + Send + Sync>,
        now_millis: u64,
    ) -> Result<(), String> {
        let memory = golem_common::serialization::serialize(&attachment)?.len()
            + golem_common::serialization::serialize(&mapping)?.len();
        let session = self.clone();
        self.producer
            .run_admitted(None, memory, false, move |_, admission| async move {
                let _session_guard = session.session_lock.lock().await;
                session
                    .activate_forwarded_mapping_under_lock(
                        &admission,
                        attachment,
                        mapping,
                        producer_control.as_ref(),
                        now_millis,
                    )
                    .await
            })
            .await
    }

    async fn activate_forwarded_mapping_under_lock(
        &self,
        admission: &Arc<StreamWriteAdmission>,
        attachment: StreamAttachmentKey,
        mapping: StreamSessionMappingRecord,
        producer_control: &(dyn StreamAttachmentControl + Send + Sync),
        now_millis: u64,
    ) -> Result<(), String> {
        self.validate_forwarded_mapping(&attachment, &mapping)?;
        if self
            .mapping_for_handle(&mapping.handle, mapping.role)
            .is_some_and(|existing| existing != mapping)
            || self
                .mapping(mapping.transport_stream_id)
                .is_some_and(|existing| existing != mapping)
        {
            return Err(
                "forwarded durable stream mapping conflicts with session topology".to_string(),
            );
        }
        let topology = self.topology_state(&attachment, Some(&mapping)).await?;
        if matches!(
            topology,
            ConsumerAttachmentStatus::IncarnationMismatch | ConsumerAttachmentStatus::EpochMismatch
        ) {
            return Err("forwarded stream attachment conflicts with durable topology".to_string());
        }
        if topology != ConsumerAttachmentStatus::Active {
            self.producer
                .ensure_session_accepts_new_events(&self.session_key)
                .await
                .map_err(|error| error.to_string())?;
        }
        if topology == ConsumerAttachmentStatus::Missing {
            self.try_append_record(
                admission,
                StreamSessionRecord::TopologyPrepared(StreamTopologyPreparedRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: self.session_key.clone(),
                    attachment: attachment.clone(),
                    mapping: mapping.clone(),
                }),
            )
            .await?;
            self.commit_consumer_journal().await?;
        }
        self.producer
            .remote_until_retired(
                producer_control.prepare_attachment(attachment.clone(), now_millis),
            )
            .await
            .map_err(|error| error.to_string())?;
        if topology != ConsumerAttachmentStatus::Active {
            self.require_local_session_attachment(&attachment).await?;
            self.try_append_record(
                admission,
                StreamSessionRecord::TopologyActivated(StreamTopologyActivatedRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: self.session_key.clone(),
                    attachment: attachment.clone(),
                    mapping: mapping.clone(),
                }),
            )
            .await?;
            self.commit_consumer_journal().await?;
        }
        self.producer
            .remote_until_retired(producer_control.activate_attachment(attachment, now_millis))
            .await
            .map_err(|error| error.to_string())?;
        let session = self.clone();
        let persisted_mapping = mapping.clone();
        admission
            .submit(move |_, context| async move {
                session
                    .append_mapping_once(&context, persisted_mapping)
                    .await
            })
            .await?;
        self.commit_consumer_journal().await?;
        self.insert_mapping(mapping)
    }

    /// Ensures an open local session is attached before producer output begins.
    pub(crate) async fn require_local_session_attachment(
        &self,
        attachment: &StreamAttachmentKey,
    ) -> Result<(), String> {
        if !self.has_local_session_authority() {
            return Ok(());
        }
        let status = self
            .oplog
            .raw_durable_stream_session_status(&self.session_key)
            .await
            .status?
            .ok_or_else(|| "durable topology has no local session authority".to_string())?;
        if let Some(error) = status.lifecycle_error {
            return Err(error);
        }
        let prepared_attempt = status
            .prepared_attempt_id
            .ok_or_else(|| "durable topology has no Prepared session authority".to_string())?;
        let (attached_epoch, attached_attempt_id, attached) = match (
            status.attachment_epoch,
            status.attachment_attempt_id,
            status.attachment_attached,
        ) {
            (Some(epoch), Some(attempt), Some(attached)) => (epoch, attempt, attached),
            _ => {
                return Err(
                    "durable topology cannot activate before session attachment".to_string()
                );
            }
        };
        if !attached
            || (attached_epoch == 1 && attached_attempt_id != prepared_attempt)
            || attached_epoch != attachment.epoch
            || AttachmentId::primary(
                self.session_key.callee_environment_id,
                &self.session_key.callee,
                &self.session_key.idempotency_key,
            )
            .map_err(|error| error.to_string())?
                != attachment.attachment_id
            || (attached_epoch == 1
                && !matches!(
                    self.oplog
                        .read(
                            status.initial_pending_invocation_oplog_index.ok_or_else(|| {
                                "durable initial attachment has no pending invocation".to_string()
                            })?
                        )
                        .await,
                    OplogEntry::PendingAgentInvocation { idempotency_key, .. }
                        if idempotency_key == self.session_key.idempotency_key
                ))
        {
            return Err(
                "durable topology attachment does not exactly match its local session authority"
                    .to_string(),
            );
        }
        Ok(())
    }

    fn has_local_session_authority(&self) -> bool {
        self.session_key.callee_environment_id == self.producer.environment_id()
            && self.session_key.callee == *self.producer.agent_id()
            && self.session_key.callee_fingerprint == self.producer.fingerprint()
    }

    fn validate_forwarded_mapping(
        &self,
        attachment: &StreamAttachmentKey,
        mapping: &StreamSessionMappingRecord,
    ) -> Result<(), String> {
        if attachment.session_key != self.session_key
            || attachment.consumer_invocation != self.consumer_invocation
            || attachment.consumer_environment_id != self.producer.environment_id()
            || attachment.consumer != *self.producer.agent_id()
            || attachment.expected_consumer_fingerprint != self.producer.fingerprint()
            || mapping.handle.stream_id != attachment.stream_id
            || mapping.handle.producer_environment_id != attachment.producer_environment_id
            || mapping.handle.producer != attachment.producer
            || mapping.handle.expected_producer_fingerprint
                != attachment.expected_producer_fingerprint
        {
            return Err(
                "forwarded stream attachment does not match the durable session or handle"
                    .to_string(),
            );
        }
        Ok(())
    }

    async fn topology_state(
        &self,
        attachment: &StreamAttachmentKey,
        expected_mapping: Option<&StreamSessionMappingRecord>,
    ) -> Result<ConsumerAttachmentStatus, String> {
        if attachment.session_key != self.session_key
            || attachment.consumer_invocation != self.consumer_invocation
            || attachment.consumer_environment_id != self.producer.environment_id()
            || attachment.consumer != *self.producer.agent_id()
            || attachment.expected_consumer_fingerprint != self.producer.fingerprint()
        {
            return Ok(ConsumerAttachmentStatus::IncarnationMismatch);
        }
        let metadata = self.current_control_metadata().await?;
        let state = metadata.topology_status(attachment, expected_mapping)?;
        if matches!(
            state,
            ConsumerAttachmentStatus::IncarnationMismatch | ConsumerAttachmentStatus::EpochMismatch
        ) {
            return Ok(state);
        }
        let local_session_authority = self.has_local_session_authority();
        let attached_epoch = metadata
            .topology_epoch
            .or_else(|| (!local_session_authority).then_some(attachment.epoch));
        match attached_epoch {
            Some(epoch) if epoch == attachment.epoch => Ok(state),
            Some(_) => Ok(ConsumerAttachmentStatus::EpochMismatch),
            None if state == ConsumerAttachmentStatus::Active => {
                Err("durable topology activation precedes session attachment".to_string())
            }
            None => Ok(state),
        }
    }

    /// Recovers durable mappings discovered inside previously journaled input values.
    pub(crate) async fn recover_nested_input_mappings(&self) -> Result<(), String> {
        self.recover_session_mappings().await
    }

    /// Reconstructs visible mappings and unresolved attachment topology from the journal.
    pub(crate) async fn recover_session_mappings(&self) -> Result<(), String> {
        // Clones share both the mapping table and its coverage. Keep the cursor locked across
        // validation so another output pump cannot observe coverage before the mappings exist.
        let mut covered = self.recovered_mappings_through.lock().await;
        let metadata = self.current_control_metadata().await?;
        let horizon = metadata.covered_through;
        let mappings: Vec<_> = metadata
            .recoverable_mappings
            .iter()
            .filter(|(index, _)| index > &*covered)
            .map(|(_, mapping)| mapping.clone())
            .collect();
        drop(metadata);
        for mapping in mappings {
            self.validate_recovered_mapping(&mapping).await?;
            self.insert_mapping(mapping)?;
        }
        *covered = horizon;
        Ok(())
    }

    /// Checks whether the consumer journal already contains a terminal for this stream.
    pub(crate) async fn has_journaled_consumer_terminal(
        &self,
        mapping: &StreamSessionMappingRecord,
    ) -> Result<bool, String> {
        let metadata = self.current_control_metadata().await?;
        if !metadata
            .closed_consumer_streams
            .contains(&mapping.handle.stream_id)
        {
            return Ok(false);
        }
        if metadata.malformed_record {
            return Err("unsupported or malformed durable Stream Session record version".into());
        }
        if let Some(error) = &metadata.topology_error {
            return Err(error.clone());
        }
        if !metadata.persisted_mappings.contains(&(
            mapping.transport_stream_id,
            mapping.handle.clone(),
            mapping.role,
        )) {
            return Err("closed consumer stream has no matching persisted mapping".into());
        }
        Ok(true)
    }

    async fn validate_recovered_mapping(
        &self,
        mapping: &StreamSessionMappingRecord,
    ) -> Result<(), String> {
        if self.producer.owns_handle_identity(&mapping.handle) {
            return self
                .producer
                .validate_handle(&mapping.handle)
                .await
                .map_err(|error| error.to_string());
        }
        if self.has_journaled_consumer_terminal(mapping).await? {
            return Ok(());
        }
        {
            let metadata = self.current_control_metadata().await?;
            if let Some(intent) = metadata.cancel_intents.get(&mapping.handle.stream_id) {
                let key = self.attachment_key(&mapping.handle, intent.epoch)?;
                if metadata.has_committed_cancellation(&key, mapping, intent)? {
                    return Ok(());
                }
            }
        }
        let attachment = self.attachment_key(&mapping.handle, self.attachment_epoch)?;
        if self.topology_state(&attachment, Some(mapping)).await?
            == ConsumerAttachmentStatus::Active
        {
            Ok(())
        } else {
            Err("foreign durable stream mapping is not topology-activated".to_string())
        }
    }

    fn attachment_key(
        &self,
        handle: &DurableStreamHandle,
        epoch: u64,
    ) -> Result<StreamAttachmentKey, String> {
        Ok(StreamAttachmentKey {
            attachment_id: golem_common::base_model::durable_stream::AttachmentId::primary(
                self.session_key.callee_environment_id,
                &self.session_key.callee,
                &self.session_key.idempotency_key,
            )
            .map_err(|error| error.to_string())?,
            stream_id: handle.stream_id,
            epoch,
            session_key: self.session_key.clone(),
            producer_environment_id: handle.producer_environment_id,
            producer: handle.producer.clone(),
            expected_producer_fingerprint: handle.expected_producer_fingerprint,
            consumer_environment_id: self.producer.environment_id(),
            consumer: self.producer.agent_id().clone(),
            expected_consumer_fingerprint: self.producer.fingerprint(),
            consumer_invocation: self.consumer_invocation.clone(),
        })
    }

    /// Rejects resume cursors that skip or contradict committed consumer progress.
    pub(crate) async fn validate_resume_cursors(
        &self,
        cursors: &[golem_common::model::durable_stream::StreamResumeCursor],
    ) -> Result<(), String> {
        for cursor in cursors {
            let mapping = self
                .mappings
                .read()
                .expect("durable stream mapping lock poisoned")
                .iter()
                .find_map(|(_, mapping)| {
                    (mapping.handle.stream_id == cursor.stream_id
                        && mapping.role == SessionStreamRole::Output)
                        .then(|| mapping.clone())
                })
                .ok_or_else(|| {
                    format!(
                        "resume cursor names no output mapping for durable stream {}",
                        cursor.stream_id
                    )
                })?;
            let Some(after) = cursor.last_observed_offset else {
                continue;
            };
            if self.producer.owns_handle_identity(&mapping.handle) {
                self.producer
                    .read_segment(&mapping.handle, Some(after), Some(after))
                    .await
                    .map_err(|error| error.to_string())?;
            } else {
                let rpc = self.rpc.clone().ok_or_else(|| {
                    "foreign durable stream source routing is unavailable".to_string()
                })?;
                let auth_ctx = self.auth_ctx.clone().ok_or_else(|| {
                    "foreign durable stream consumer authorization is unavailable".to_string()
                })?;
                let source = RoutedAttachedStreamSegmentSource::new(
                    rpc,
                    mapping.clone(),
                    auth_ctx,
                    self.producer.clone(),
                );
                source
                    .read_attached_segment(
                        &self.attachment_key(&mapping.handle, self.attachment_epoch)?,
                        &mapping.handle,
                        Timestamp::now_utc().to_millis(),
                        Some(after),
                        Some(after),
                    )
                    .await
                    .map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    }

    /// Commits activation of a prepared cross-agent mapping and its attachment epoch.
    pub(crate) async fn activate_foreign_mapping(
        &self,
        mapping: StreamSessionMappingRecord,
        epoch: u64,
    ) -> Result<(), String> {
        if self.producer.owns_handle_identity(&mapping.handle) {
            return Err("foreign durable mapping is owned by the local producer".to_string());
        }
        let rpc = self
            .rpc
            .clone()
            .ok_or_else(|| "foreign durable stream control routing is unavailable".to_string())?;
        let auth_ctx = self.auth_ctx.clone().ok_or_else(|| {
            "foreign durable stream consumer authorization is unavailable".to_string()
        })?;
        let attachment = self.attachment_key(&mapping.handle, epoch)?;
        let control = RoutedStreamAttachmentControl::new(rpc, mapping.clone(), auth_ctx);
        self.activate_forwarded_mapping(
            attachment,
            mapping,
            Arc::new(control),
            Timestamp::now_utc().to_millis(),
        )
        .await
    }

    /// Prepares the producer attachment before recording the consumer topology.
    pub(crate) async fn prepare_foreign_mapping(
        &self,
        mapping: StreamSessionMappingRecord,
        epoch: u64,
    ) -> Result<(), String> {
        let memory = golem_common::serialization::serialize(&mapping)?.len();
        let session = self.clone();
        self.producer
            .run_admitted(None, memory, false, move |_, admission| async move {
                session
                    .prepare_foreign_mapping_owned(&admission, mapping, epoch)
                    .await
            })
            .await
    }

    async fn prepare_foreign_mapping_owned(
        &self,
        admission: &Arc<StreamWriteAdmission>,
        mapping: StreamSessionMappingRecord,
        epoch: u64,
    ) -> Result<(), String> {
        if self.producer.owns_handle_identity(&mapping.handle) {
            return Err("foreign durable mapping is owned by the local producer".to_string());
        }
        let rpc = self
            .rpc
            .clone()
            .ok_or_else(|| "foreign durable stream control routing is unavailable".to_string())?;
        let auth_ctx = self.auth_ctx.clone().ok_or_else(|| {
            "foreign durable stream consumer authorization is unavailable".to_string()
        })?;
        let attachment = self.attachment_key(&mapping.handle, epoch)?;
        let control = RoutedStreamAttachmentControl::new(rpc, mapping.clone(), auth_ctx);
        let _session_guard = self.session_lock.lock().await;
        self.validate_forwarded_mapping(&attachment, &mapping)?;
        if self
            .mapping_for_handle(&mapping.handle, mapping.role)
            .is_some_and(|existing| existing != mapping)
            || self
                .mapping(mapping.transport_stream_id)
                .is_some_and(|existing| existing != mapping)
        {
            return Err(
                "forwarded durable stream mapping conflicts with session topology".to_string(),
            );
        }
        let topology = self.topology_state(&attachment, Some(&mapping)).await?;
        if matches!(
            topology,
            ConsumerAttachmentStatus::IncarnationMismatch | ConsumerAttachmentStatus::EpochMismatch
        ) {
            return Err("forwarded stream attachment conflicts with durable topology".to_string());
        }
        if topology == ConsumerAttachmentStatus::Active {
            return Ok(());
        }
        if topology == ConsumerAttachmentStatus::Missing {
            if !self.persisted_session_mapping(&mapping).await? {
                self.producer
                    .ensure_session_accepts_new_events(&self.session_key)
                    .await
                    .map_err(|error| error.to_string())?;
            }
            self.try_append_record(
                admission,
                StreamSessionRecord::TopologyPrepared(StreamTopologyPreparedRecord {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: self.session_key.clone(),
                    attachment: attachment.clone(),
                    mapping,
                }),
            )
            .await?;
            self.commit_consumer_journal().await?;
        }
        self.producer
            .remote_until_retired(
                control.prepare_attachment(attachment, Timestamp::now_utc().to_millis()),
            )
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    async fn persisted_session_mapping(
        &self,
        expected: &StreamSessionMappingRecord,
    ) -> Result<bool, String> {
        Ok(self
            .current_control_metadata()
            .await?
            .persisted_mappings
            .contains(&(
                expected.transport_stream_id,
                expected.handle.clone(),
                expected.role,
            )))
    }

    async fn ensure_nested_mapping(
        &self,
        context: Option<&Arc<StreamWriteAdmission>>,
        handle: DurableStreamHandle,
        role: SessionStreamRole,
    ) -> Result<StreamSessionMappingRecord, String> {
        let memory = golem_common::serialization::serialize(&handle)?.len();
        let session = self.clone();
        self.producer
            .run_admitted(context, memory, false, move |_, admission| async move {
                let _session_guard = session.session_lock.lock().await;
                session
                    .ensure_nested_mapping_under_lock(&admission, handle, role)
                    .await
            })
            .await
    }

    async fn ensure_nested_mapping_under_lock(
        &self,
        admission: &Arc<StreamWriteAdmission>,
        handle: DurableStreamHandle,
        role: SessionStreamRole,
    ) -> Result<StreamSessionMappingRecord, String> {
        if let Some(mapping) = self.mapping_for_handle(&handle, role) {
            return Ok(mapping);
        }
        let mapping = StreamSessionMappingRecord {
            transport_stream_id: self.allocate_transport_stream_id()?,
            handle,
            role,
        };
        if self.producer.owns_handle_identity(&mapping.handle) {
            let session = self.clone();
            let persisted_mapping = mapping.clone();
            admission
                .submit(move |_, context| async move {
                    session
                        .append_mapping_once(&context, persisted_mapping)
                        .await
                })
                .await?;
            self.insert_mapping(mapping.clone())?;
        } else {
            let rpc = self.rpc.clone().ok_or_else(|| {
                "foreign durable stream control routing is unavailable".to_string()
            })?;
            let auth_ctx = self.auth_ctx.clone().ok_or_else(|| {
                "foreign durable stream consumer authorization is unavailable".to_string()
            })?;
            let attachment = self.attachment_key(&mapping.handle, self.attachment_epoch)?;
            let control = RoutedStreamAttachmentControl::new(rpc, mapping.clone(), auth_ctx);
            self.activate_forwarded_mapping_under_lock(
                admission,
                attachment,
                mapping.clone(),
                &control,
                Timestamp::now_utc().to_millis(),
            )
            .await?;
        }
        Ok(mapping)
    }

    /// Attaches a fingerprint-validated foreign handle and returns its transport mapping.
    pub(crate) async fn attach_foreign_handle(
        &self,
        handle: DurableStreamHandle,
        role: SessionStreamRole,
        epoch: u64,
    ) -> Result<StreamSessionMappingRecord, String> {
        if self.producer.owns_handle_identity(&handle) {
            return Err("attached durable stream handle is owned by the consumer".to_string());
        }
        if let Some(mapping) = self.mapping_for_handle(&handle, role) {
            return Ok(mapping);
        }
        let mapping = StreamSessionMappingRecord {
            transport_stream_id: self.allocate_transport_stream_id()?,
            handle,
            role,
        };
        self.activate_foreign_mapping(mapping.clone(), epoch)
            .await?;
        Ok(mapping)
    }

    /// Validates, commits, and acknowledges one input frame for the current attachment.
    pub(crate) async fn write_input(
        &self,
        context: Option<&Arc<StreamWriteAdmission>>,
        transport_stream_id: u64,
        first_sequence: u64,
        payload: StreamItemsPayload,
    ) -> Result<
        Option<(
            u64,
            u64,
            golem_common::model::durable_stream::StreamOffset,
            Vec<StreamSessionMappingRecord>,
        )>,
        String,
    > {
        let memory = DurableStreamStore::retained_payload_bytes(&payload)?;
        let session = self.clone();
        self.producer
            .run_admitted(context, memory, false, move |_, admission| async move {
                let _session_guard = session.session_lock.lock().await;
                let session_for_write = session.clone();
                admission
                    .submit(move |_, context| async move {
                        session_for_write
                            .write_input_owned(
                                &context,
                                transport_stream_id,
                                first_sequence,
                                payload,
                            )
                            .await
                    })
                    .await
            })
            .await
    }

    async fn write_input_owned(
        &self,
        context: &StreamWriteContext,
        transport_stream_id: u64,
        first_sequence: u64,
        payload: StreamItemsPayload,
    ) -> Result<
        Option<(
            u64,
            u64,
            golem_common::model::durable_stream::StreamOffset,
            Vec<StreamSessionMappingRecord>,
        )>,
        String,
    > {
        self.ensure_current_attachment().await?;
        let handle = self
            .handle(transport_stream_id)
            .ok_or_else(|| format!("unknown durable input stream {transport_stream_id}"))?;
        let logical_item_count = u64::try_from(payload.logical_item_count())
            .map_err(|_| "durable input item count does not fit in u64".to_string())?;
        let mut nested_transport_ids = Vec::new();
        let mut nested_requests = Vec::new();
        let mut nested_element_types = Vec::new();
        if let StreamItemsPayload::Values(values) = &payload {
            for (item_index, value) in values.iter().enumerate() {
                let value = ProtoSchemaValue::decode(value.as_slice())
                    .map_err(|error| format!("invalid durable input value: {error}"))?;
                let input_schema = self.input_schema.as_ref().ok_or_else(|| {
                    "nested durable input streams require the persisted input schema".to_string()
                })?;
                let parent_element = input_schema
                    .element_types
                    .read()
                    .expect("durable input schema lock poisoned")
                    .get(&transport_stream_id)
                    .cloned()
                    .ok_or_else(|| {
                        format!("missing element schema for durable input {transport_stream_id}")
                    })?;
                let nested = collect_stream_paths(&value, &input_schema.graph, &parent_element)?;
                if nested.is_empty() {
                    continue;
                }
                let parent_producer_sequence = first_sequence
                    .checked_add(item_index as u64)
                    .ok_or_else(|| "durable input sequence overflow".to_string())?;
                for (nested_transport_id, path) in nested {
                    if nested_transport_ids.contains(&nested_transport_id) {
                        return Err(format!(
                            "duplicate nested durable transport stream id {nested_transport_id}"
                        ));
                    }
                    let element =
                        stream_element_schema(&input_schema.graph, &parent_element, &path)?
                            .cloned();
                    let element_schema_fingerprint =
                        schema_fingerprint_v1(&input_schema.graph, element.as_ref())
                            .map_err(|error| error.to_string())?;
                    let coordinate = StreamRegistrationCoordinate::Nested {
                        parent_stream_id: handle.stream_id,
                        parent_producer_sequence,
                        recursive_value_path: path,
                    };
                    if let Some(existing) = self.handle(nested_transport_id) {
                        let global_parent_sequence = self
                            .producer
                            .attached_global_sequence(
                                &self.session_key,
                                handle.stream_id,
                                first_sequence,
                            )
                            .await
                            .map_err(|error| error.to_string())?
                            .checked_add(item_index as u64)
                            .ok_or_else(|| "durable input sequence overflow".to_string())?;
                        let mut global_coordinate = coordinate.clone();
                        if let StreamRegistrationCoordinate::Nested {
                            parent_producer_sequence,
                            ..
                        } = &mut global_coordinate
                        {
                            *parent_producer_sequence = global_parent_sequence;
                        }
                        if self
                            .producer
                            .handle_for_coordinate(&global_coordinate)
                            .await
                            .map_err(|error| error.to_string())?
                            .as_ref()
                            != Some(&existing)
                        {
                            return Err(format!(
                                "nested durable transport stream id {nested_transport_id} conflicts with its persisted coordinate"
                            ));
                        }
                    }
                    nested_transport_ids.push(nested_transport_id);
                    nested_element_types
                        .push((nested_transport_id, element.unwrap_or_else(SchemaType::u8)));
                    nested_requests.push(ProducerRegistrationRequest {
                        entity_parent_start_index: self.entity_parent_start_index,
                        coordinate,
                        source_invocation: self.session_key.clone(),
                        component_revision: input_schema.component_revision,
                        element_schema_fingerprint,
                        source_kind: StreamSourceKind::Nested,
                        session_mapping: None,
                    });
                }
            }
        }
        let canonical_payload = match &payload {
            StreamItemsPayload::Values(values) => {
                let mut next_handle_index = 0usize;
                let mut canonical_values = Vec::with_capacity(values.len());
                for value in values {
                    let value = ProtoSchemaValue::decode(value.as_slice())
                        .map_err(|error| format!("invalid durable input value: {error}"))?;
                    let value =
                        remap_recursive_stream_references(value, |transport_stream_id, _| {
                            let expected = nested_transport_ids
                                .get(next_handle_index)
                                .copied()
                                .ok_or_else(|| {
                                    "durable input contains an unexpected nested stream".to_string()
                                })?;
                            if expected != transport_stream_id {
                                return Err(
                                    "durable input stream traversal changed during canonicalization"
                                        .to_string(),
                                );
                            }
                            let canonical_index = u64::try_from(next_handle_index)
                                .map_err(|_| "durable input handle index overflow".to_string())?;
                            next_handle_index += 1;
                            Ok(canonical_index)
                        })?;
                    canonical_values.push(value.encode_to_vec());
                }
                if next_handle_index != nested_transport_ids.len() {
                    return Err(
                        "durable input stream topology changed during canonicalization".to_string(),
                    );
                }
                StreamItemsPayload::Values(canonical_values)
            }
            StreamItemsPayload::PackedU8(bytes) => StreamItemsPayload::PackedU8(bytes.clone()),
        };
        let outcome = match self
            .producer
            .write_attached_items_with_nested(
                Some(context),
                &self.session_key,
                handle.stream_id,
                first_sequence,
                canonical_payload,
                nested_requests,
            )
            .await
        {
            Ok(outcome) => outcome,
            Err(error) if discards_input_after_terminal(&error, &self.session_key) => {
                tracing::debug!(
                    transport_stream_id,
                    first_sequence,
                    error = %error,
                    "Discarding late durable input items after terminal"
                );
                return Ok(None);
            }
            Err(error) => {
                tracing::warn!(
                    transport_stream_id,
                    first_sequence,
                    error = %error,
                    "Rejecting durable input items"
                );
                return Err(error.to_string());
            }
        };
        let mut nested_mappings = Vec::with_capacity(nested_transport_ids.len());
        if !nested_transport_ids.is_empty() {
            let global_first_sequence = self
                .producer
                .attached_global_sequence(&self.session_key, handle.stream_id, first_sequence)
                .await
                .map_err(|error| error.to_string())?;
            let nested_handles = self
                .producer
                .nested_handles(handle.stream_id, global_first_sequence)
                .await
                .map_err(|error| error.to_string())?;
            if nested_handles.len() != nested_transport_ids.len() {
                return Err(
                    "nested stream mapping count does not match durable item metadata".to_string(),
                );
            }
            for (transport_stream_id, handle) in
                nested_transport_ids.into_iter().zip(nested_handles)
            {
                let mapping = StreamSessionMappingRecord {
                    transport_stream_id,
                    handle: handle.clone(),
                    role: SessionStreamRole::Input,
                };
                self.insert_mapping(mapping.clone())?;
                nested_mappings.push(mapping);
            }
            for mapping in &nested_mappings {
                self.append_mapping_once(context, mapping.clone()).await?;
            }
            let input_schema = self
                .input_schema
                .as_ref()
                .expect("nested input schema was validated before durable commit");
            input_schema
                .element_types
                .write()
                .expect("durable input schema lock poisoned")
                .extend(nested_element_types);
        }
        let resulting_offset = outcome
            .value
            .last()
            .copied()
            .ok_or_else(|| "durable input batch contains no logical items".to_string())?;
        let highest_contiguous_sequence = first_sequence
            .checked_add(logical_item_count - 1)
            .ok_or_else(|| "durable input sequence overflow".to_string())?;
        Ok(Some((
            highest_contiguous_sequence,
            logical_item_count,
            resulting_offset,
            nested_mappings,
        )))
    }

    /// Commits the input terminal for the current attachment exactly once.
    pub(crate) async fn end_input(
        &self,
        transport_stream_id: u64,
        sequence: u64,
    ) -> Result<Option<golem_common::model::durable_stream::StreamOffset>, String> {
        let session = self.clone();
        self.producer
            .run_admitted(None, 0, false, move |_, admission| async move {
                let _session_guard = session.session_lock.lock().await;
                let session_for_write = session.clone();
                admission
                    .submit(move |_, context| async move {
                        session_for_write
                            .end_input_owned(&context, transport_stream_id, sequence)
                            .await
                    })
                    .await
            })
            .await
    }

    async fn end_input_owned(
        &self,
        context: &StreamWriteContext,
        transport_stream_id: u64,
        sequence: u64,
    ) -> Result<Option<golem_common::model::durable_stream::StreamOffset>, String> {
        self.ensure_current_attachment().await?;
        let handle = self
            .handle(transport_stream_id)
            .ok_or_else(|| format!("unknown durable input stream {transport_stream_id}"))?;
        match self
            .producer
            .append_external_input(
                Some(context),
                &self.session_key,
                handle.stream_id,
                None,
                true,
                Some(super::durable_stream::ExternalProducer {
                    id: golem_common::base_model::durable_stream::ExternalProducerId::Attached,
                    epoch: 0,
                    sequence,
                }),
            )
            .await
        {
            Ok(
                super::durable_stream::ExternalAppendOutcome::Accepted(offset)
                | super::durable_stream::ExternalAppendOutcome::Duplicate { offset, .. },
            ) => Ok(Some(offset)),
            Ok(super::durable_stream::ExternalAppendOutcome::Closed) => Ok(None),
            Ok(outcome) => Err(format!(
                "unexpected attached input end outcome: {outcome:?}"
            )),
            Err(error) if discards_input_after_terminal(&error, &self.session_key) => {
                tracing::debug!(
                    transport_stream_id,
                    sequence,
                    error = %error,
                    "Discarding late durable input end after terminal"
                );
                Ok(None)
            }
            Err(error) => {
                tracing::warn!(
                    transport_stream_id,
                    sequence,
                    error = %error,
                    "Rejecting durable input end"
                );
                Err(error.to_string())
            }
        }
    }

    /// Durably requests cancellation for all still-open streams in the session.
    pub(crate) async fn cancel_session_streams(&self) -> Result<bool, String> {
        if !self.has_local_session_authority() {
            return Err("session cancellation requires the session owner".into());
        }
        let session = self.clone();
        self.producer
            .run_admitted(None, 0, true, move |_, admission| async move {
                let guard = session.session_lock.lock().await;
                let metadata = session.current_control_metadata().await?;
                if metadata.prepared.is_none() {
                    return Ok::<_, String>(false);
                }
                if metadata.malformed_record || metadata.topology_error.is_some() {
                    return Err("cannot cancel a malformed durable stream session".into());
                }
                let (epoch, _, _) = session.authoritative_attachment_state().await?;
                let mut records = Vec::new();
                if !metadata.cancellation_requested {
                    records.push(StreamSessionRecord::CancelRequested(
                        StreamSessionCancelRequestedRecord {
                            format_version: DURABLE_STREAM_FORMAT_VERSION,
                            session_key: session.session_key.clone(),
                        },
                    ));
                }
                let mut cancelled = metadata
                    .cancel_intents
                    .keys()
                    .copied()
                    .collect::<HashSet<_>>();
                for (_, handle, role) in &metadata.persisted_mappings {
                    if cancelled.insert(handle.stream_id) {
                        records.push(StreamSessionRecord::ConsumerCancelIntent(
                            StreamConsumerCancelIntentRecord {
                                format_version: DURABLE_STREAM_FORMAT_VERSION,
                                session_key: session.session_key.clone(),
                                stream_id: handle.stream_id,
                                epoch,
                                role: match role {
                                    SessionStreamRole::Input => StreamCancelRole::InputProducer,
                                    SessionStreamRole::Output => StreamCancelRole::OutputConsumer,
                                },
                                reason: StreamCancelReason::Cancelled,
                                details: None,
                            },
                        ));
                    }
                }
                drop(metadata);
                if !records.is_empty() {
                    let attribution = session.entity_parent_start_index;
                    admission
                        .submit(move |owner, context| async move {
                            owner
                                .append_session_records_owned(&context, attribution, records)
                                .await
                        })
                        .await
                        .map_err(|error| error.to_string())?;
                }
                drop(guard);
                session
                    .reconcile_local_cancellation_intents(Some(&admission))
                    .await?;
                Ok(true)
            })
            .await
    }

    /// The caller resolves the canonical slot under this session guard inside an owned lifecycle operation.
    pub(crate) async fn tombstone_slot_owned(
        &self,
        admission: &Arc<StreamWriteAdmission>,
        slot: String,
        stream: Option<(DurableStreamHandle, SessionStreamRole)>,
        session_guard: tokio::sync::OwnedMutexGuard<()>,
    ) -> Result<bool, String> {
        if !self.has_local_session_authority() {
            return Err("slot deletion requires the session owner".into());
        }
        let metadata = self.current_control_metadata().await?;
        if metadata.prepared.is_none() {
            return Err("unknown durable stream session".into());
        }
        if metadata.tombstoned_slots.contains_key(&slot) {
            return Ok(false);
        }
        let mut records = vec![StreamSessionRecord::Tombstoned(
            StreamSlotTombstonedRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: self.session_key.clone(),
                slot,
                role: stream
                    .as_ref()
                    .map_or(SessionStreamRole::Output, |(_, role)| *role),
            },
        )];
        if let Some((handle, role)) = stream {
            if !metadata
                .persisted_mappings
                .iter()
                .any(|(_, saved, saved_role)| saved == &handle && saved_role == &role)
            {
                return Err("deleted slot has no persisted stream mapping".into());
            }
            if !metadata.cancel_intents.contains_key(&handle.stream_id) {
                let (epoch, _, _) = self.authoritative_attachment_state().await?;
                records.push(StreamSessionRecord::ConsumerCancelIntent(
                    StreamConsumerCancelIntentRecord {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: self.session_key.clone(),
                        stream_id: handle.stream_id,
                        epoch,
                        role: match role {
                            SessionStreamRole::Input => StreamCancelRole::InputProducer,
                            SessionStreamRole::Output => StreamCancelRole::OutputConsumer,
                        },
                        reason: StreamCancelReason::Cancelled,
                        details: None,
                    },
                ));
            }
        }
        drop(metadata);
        let attribution = self.entity_parent_start_index;
        admission
            .submit(move |owner, context| async move {
                owner
                    .append_session_records_owned(&context, attribution, records)
                    .await
            })
            .await
            .map_err(|error| error.to_string())?;
        drop(session_guard);
        self.reconcile_local_cancellation_intents(Some(admission))
            .await?;
        Ok(true)
    }

    /// Journals consumer cancellation before forwarding it to the producer.
    pub(crate) async fn cancel_stream(
        &self,
        transport_stream_id: u64,
        role: StreamCancelRole,
        reason: StreamCancelReason,
        details: Option<String>,
        expected_attachment_epoch: Option<u64>,
    ) -> Result<(), String> {
        let session = self.clone();
        self.producer
            .run_admitted(
                None,
                details.as_ref().map_or(0, String::len),
                true,
                move |_, admission| async move {
                    session
                        .cancel_stream_owned(
                            &admission,
                            transport_stream_id,
                            role,
                            reason,
                            details,
                            expected_attachment_epoch,
                        )
                        .await
                },
            )
            .await
    }

    async fn cancel_stream_owned(
        &self,
        admission: &Arc<StreamWriteAdmission>,
        transport_stream_id: u64,
        role: StreamCancelRole,
        reason: StreamCancelReason,
        details: Option<String>,
        expected_attachment_epoch: Option<u64>,
    ) -> Result<(), String> {
        let session_guard = self.session_lock.lock().await;
        if let Some(expected_epoch) = expected_attachment_epoch {
            if expected_epoch != self.attachment_epoch {
                return Err("StaleEpoch: durable stream cancellation was fenced".to_string());
            }
            self.ensure_current_attachment().await?;
        }
        let mapping = self
            .mapping(transport_stream_id)
            .ok_or_else(|| format!("unknown durable input stream {transport_stream_id}"))?;
        let expected_role = match role {
            StreamCancelRole::InputProducer | StreamCancelRole::InputConsumer => {
                SessionStreamRole::Input
            }
            StreamCancelRole::OutputProducer | StreamCancelRole::OutputConsumer => {
                SessionStreamRole::Output
            }
            StreamCancelRole::System => {
                return Err("system-authored durable stream cancellation is internal".to_string());
            }
        };
        if mapping.role != expected_role {
            return Err("durable stream cancellation role does not match its mapping".to_string());
        }
        let epoch = if self.has_local_session_authority() {
            match self.attachment_attempt_id {
                // Streams bound to a transport attempt may only cancel while that attempt is
                // still the current attachment; a superseded attempt is fenced.
                Some(_) => {
                    self.ensure_current_attachment().await?;
                    self.attachment_epoch
                }
                // Streams handed to the callee guest are not bound to any transport attempt:
                // the guest owns the consumer side for the whole invocation, so its cancellation
                // targets whatever attachment epoch is currently authoritative.
                None => {
                    let (epoch, _, _) = self.authoritative_attachment_state().await?;
                    epoch
                }
            }
        } else {
            self.validate_recovered_mapping(&mapping).await?;
            self.attachment_epoch
        };
        let intent = StreamConsumerCancelIntentRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            session_key: self.session_key.clone(),
            stream_id: mapping.handle.stream_id,
            epoch,
            role,
            reason,
            details,
        };
        let persisted_intent = self
            .current_control_metadata()
            .await?
            .cancel_intents
            .get(&mapping.handle.stream_id)
            .cloned();
        let intent = match persisted_intent {
            Some(existing) => existing,
            None => {
                self.try_append_record(
                    admission,
                    StreamSessionRecord::ConsumerCancelIntent(intent.clone()),
                )
                .await?;
                self.commit_consumer_journal().await?;
                intent
            }
        };
        self.apply_cancel_intent_owned(admission, mapping, intent, session_guard)
            .await
    }

    /// Applies committed cancellation intents to streams produced by the local agent.
    pub(crate) async fn reconcile_local_cancellation_intents(
        &self,
        admission: Option<&Arc<StreamWriteAdmission>>,
    ) -> Result<(), String> {
        let metadata = self.current_control_metadata().await?;
        let mut pending = Vec::new();
        for intent in metadata.cancel_intents.values() {
            if metadata.applied_cancel_intents.contains(intent) {
                continue;
            }
            let (transport_stream_id, handle, role) = metadata
                .persisted_mappings
                .iter()
                .find(|(_, handle, _)| handle.stream_id == intent.stream_id)
                .ok_or("durable cancellation intent has no persisted stream mapping")?;
            if self.producer.owns_handle_identity(handle) {
                pending.push((
                    StreamSessionMappingRecord {
                        transport_stream_id: *transport_stream_id,
                        handle: handle.clone(),
                        role: *role,
                    },
                    intent.clone(),
                ));
            }
        }
        drop(metadata);
        for (mapping, intent) in pending {
            let session = self.clone();
            self.producer
                .run_admitted(
                    admission,
                    intent.details.as_ref().map_or(0, String::len),
                    true,
                    move |_, admission| async move {
                        let _guard = session.session_lock.lock().await;
                        let session_for_write = session.clone();
                        admission
                            .submit(move |_, context| async move {
                                let session = session_for_write;
                                session
                                    .producer
                                    .validate_handle(&mapping.handle)
                                    .await
                                    .map_err(|error| error.to_string())?;
                                // The terminal dispatcher owns publication; recovery only waits for
                                // durable cancellation, not for a slow live reader to make room.
                                session
                                    .producer
                                    .commit_cancel_open(
                                        Some(&context),
                                        mapping.handle.stream_id,
                                        intent.role,
                                        intent.reason,
                                        intent.details.clone(),
                                    )
                                    .await
                                    .map_err(|error| error.to_string())?;
                                session
                                    .producer
                                    .append_session_record_owned(
                                        &context,
                                        session.entity_parent_start_index,
                                        StreamSessionRecord::ConsumerCancelApplied(
                                            StreamConsumerCancelAppliedRecord {
                                                format_version: DURABLE_STREAM_FORMAT_VERSION,
                                                intent,
                                            },
                                        ),
                                    )
                                    .await
                                    .map_err(|error| error.to_string())?;
                                Ok::<_, String>(())
                            })
                            .await
                    },
                )
                .await?;
        }
        Ok(())
    }

    /// Forwards committed cancellation intents and records their durable application.
    pub(crate) async fn reconcile_foreign_cancellation_intents(
        &self,
        timeout: Duration,
    ) -> Result<(), String> {
        let metadata = self.current_control_metadata().await?;
        let mut pending = Vec::new();
        for intent in metadata.cancel_intents.values() {
            if metadata.applied_cancel_intents.contains(intent) {
                continue;
            }
            let (transport_stream_id, handle, role) = metadata
                .persisted_mappings
                .iter()
                .find(|(_, handle, _)| handle.stream_id == intent.stream_id)
                .ok_or("durable cancellation intent has no persisted stream mapping")?;
            if self.producer.owns_handle_identity(handle) {
                continue;
            }
            let mapping = StreamSessionMappingRecord {
                transport_stream_id: *transport_stream_id,
                handle: handle.clone(),
                role: *role,
            };
            let consumer_invocation = if self.has_local_session_authority() {
                self.session_key.clone()
            } else {
                metadata
                    .topologies
                    .values()
                    .find(|topology| topology.mapping == mapping)
                    .ok_or("foreign cancellation has no consumer invocation authority")?
                    .attachment
                    .consumer_invocation
                    .clone()
            };
            let key = self
                .clone()
                .with_consumer_invocation(consumer_invocation)
                .attachment_key(handle, intent.epoch)?;
            pending.push((key, mapping, intent.clone()));
        }
        drop(metadata);
        let mut first_error = None;
        for (key, mapping, intent) in pending {
            let rpc = self
                .rpc
                .clone()
                .ok_or("foreign cancellation routing is unavailable")?;
            let auth_ctx = self
                .auth_ctx
                .clone()
                .ok_or("foreign cancellation authorization is unavailable")?;
            let receipt_intent = intent.clone();
            let attribution = self.entity_parent_start_index;
            let result = self
                .producer
                .run_admitted(
                    None,
                    intent.details.as_ref().map_or(0, String::len),
                    true,
                    move |_, admission| async move {
                        tokio::time::timeout(
                            timeout,
                            RoutedStreamAttachmentControl::new(rpc, mapping, auth_ctx)
                                .cancel_stream(key, intent.role, intent.reason, intent.details),
                        )
                        .await
                        .map_err(|_| StreamStoreError::RecoveryRequired)?
                        .map_err(String::from)?;
                        admission
                            .submit(move |owner, context| async move {
                                owner
                                    .append_session_record_owned(
                                        &context,
                                        attribution,
                                        StreamSessionRecord::ConsumerCancelApplied(
                                            StreamConsumerCancelAppliedRecord {
                                                format_version: DURABLE_STREAM_FORMAT_VERSION,
                                                intent: receipt_intent,
                                            },
                                        ),
                                    )
                                    .await
                                    .map_err(|error| error.to_string())
                            })
                            .await
                    },
                )
                .await;
            if let Err(error) = result {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    async fn apply_cancel_intent_owned(
        &self,
        admission: &Arc<StreamWriteAdmission>,
        mapping: StreamSessionMappingRecord,
        intent: StreamConsumerCancelIntentRecord,
        session_guard: tokio::sync::MutexGuard<'_, ()>,
    ) -> Result<(), String> {
        if self
            .current_control_metadata()
            .await?
            .applied_cancel_intents
            .contains(&intent)
        {
            return Ok(());
        }
        if self.producer.owns_handle_identity(&mapping.handle) {
            let attribution = self.entity_parent_start_index;
            admission
                .submit(move |owner, context| async move {
                    let pending = owner
                        .commit_cancel_open(
                            Some(&context),
                            mapping.handle.stream_id,
                            intent.role,
                            intent.reason,
                            intent.details.clone(),
                        )
                        .await
                        .map_err(|error| error.to_string())?;
                    owner
                        .append_session_record_owned(
                            &context,
                            attribution,
                            StreamSessionRecord::ConsumerCancelApplied(
                                StreamConsumerCancelAppliedRecord {
                                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                                    intent: intent.clone(),
                                },
                            ),
                        )
                        .await
                        .map_err(|error| error.to_string())?;
                    if let Some(pending) = pending {
                        owner
                            .publish_committed_cancellation(Some(&context), pending)
                            .await
                            .map_err(|error| error.to_string())?;
                    }
                    Ok::<_, String>(())
                })
                .await?;
            drop(session_guard);
        } else {
            drop(session_guard);
            let rpc = self.rpc.clone().ok_or_else(|| {
                "foreign durable stream cancellation routing is unavailable".to_string()
            })?;
            let auth_ctx = self.auth_ctx.clone().ok_or_else(|| {
                "foreign durable stream cancellation authorization is unavailable".to_string()
            })?;
            let key = self.attachment_key(&mapping.handle, intent.epoch)?;
            self.producer
                .remote_until_retired(
                    RoutedStreamAttachmentControl::new(rpc, mapping, auth_ctx).cancel_stream(
                        key,
                        intent.role,
                        intent.reason,
                        intent.details,
                    ),
                )
                .await
                .map_err(String::from)?;
        }
        Ok(())
    }

    /// Returns committed resume boundaries for every mapped input stream.
    pub(crate) async fn input_high_waters(
        &self,
    ) -> Result<HashMap<u64, InputStreamHighWater>, String> {
        let mappings = self
            .mappings
            .read()
            .expect("durable stream mapping lock poisoned")
            .clone();
        let mut result = HashMap::new();
        for (transport_stream_id, mapping) in mappings {
            if mapping.role != SessionStreamRole::Input
                || !self.producer.owns_handle_identity(&mapping.handle)
            {
                continue;
            }
            if let Some(high_water) = self
                .producer
                .attached_input_high_water(&self.session_key, mapping.handle.stream_id)
                .await
                .map_err(|error| error.to_string())?
            {
                result.insert(transport_stream_id, high_water);
            }
        }
        Ok(result)
    }

    /// Replaces input stream handles with consumers backed by this session journal.
    pub(crate) async fn materialize_agent_input(
        &self,
        value: &SchemaValue,
        graph: &SchemaGraph,
        root: &SchemaType,
        component_revision: golem_common::model::component::ComponentRevision,
    ) -> Result<(ProtoSchemaValue, Vec<StreamSessionMappingRecord>), String> {
        preflight_recursive_stream_value(value)?;
        let mut next_stream_index = 0u64;
        let retained_bytes =
            encode_recursive_stream_value_with_schema(value, graph, root, |_stream, _path| {
                let index = next_stream_index;
                next_stream_index = next_stream_index
                    .checked_add(1)
                    .ok_or_else(|| "durable input handle index overflow".to_string())?;
                Ok(index)
            })?
            .encoded_len();
        let session = self.clone();
        let value = value.clone();
        let graph = graph.clone();
        let root = root.clone();
        self.producer
            .run_admitted(
                None,
                retained_bytes,
                false,
                move |_, admission| async move {
                    let _guard = session.session_lock.lock().await;
                    let session_for_write = session.clone();
                    admission
                        .submit(move |_, context| async move {
                            session_for_write
                                .materialize_agent_input_owned(
                                    &context,
                                    value,
                                    graph,
                                    root,
                                    component_revision,
                                )
                                .await
                        })
                        .await
                },
            )
            .await
    }

    async fn materialize_agent_input_owned(
        &self,
        context: &StreamWriteContext,
        value: SchemaValue,
        graph: SchemaGraph,
        root: SchemaType,
        component_revision: golem_common::model::component::ComponentRevision,
    ) -> Result<(ProtoSchemaValue, Vec<StreamSessionMappingRecord>), String> {
        struct PendingInput {
            path: Vec<StreamValuePathStep>,
            endpoint: Option<LiveStreamEndpoint>,
            forwarded_handle: Option<DurableStreamHandle>,
            element_type: SchemaType,
            element_schema_fingerprint: SchemaFingerprintV1,
        }

        self.recover_session_mappings().await?;
        validate_forwarded_durable_input_schemas(
            &value,
            &graph,
            &root,
            "forwarded durable input handle does not match the invocation stream schema",
        )?;
        let mut pending = Vec::new();
        let encoded =
            encode_recursive_stream_value_with_schema(&value, &graph, &root, |stream, path| {
                let element = stream_element_schema(&graph, &root, path)?;
                let element_schema_fingerprint =
                    schema_fingerprint_v1(&graph, element).map_err(|error| error.to_string())?;
                let forwarded = forwarded_durable_input_reference(stream)?;
                let (endpoint, forwarded_handle) = match forwarded {
                    Some(forwarded) => (None, Some(forwarded.take(stream)?.handle)),
                    None => (
                        Some(stream.take_host_endpoint::<LiveStreamEndpoint>()?),
                        None,
                    ),
                };
                pending.push(PendingInput {
                    path: path.to_vec(),
                    endpoint,
                    forwarded_handle,
                    element_type: element.cloned().unwrap_or_else(SchemaType::u8),
                    element_schema_fingerprint,
                });
                u64::try_from(pending.len() - 1)
                    .map_err(|_| "durable input handle index overflow".to_string())
            })?;
        self.producer
            .validate_new_session_stream_count(&self.session_key, pending.len())
            .await
            .map_err(|error| error.to_string())?;

        let session_mapping = StreamSessionMapping {
            session_key: self.session_key.clone(),
            attachment_id: golem_common::model::durable_stream::AttachmentId::primary(
                self.session_key.callee_environment_id,
                &self.session_key.callee,
                &self.session_key.idempotency_key,
            )
            .map_err(|error| error.to_string())?,
            role: SessionStreamRole::Input,
        };
        let mut mappings = Vec::with_capacity(pending.len());
        let mut drains = Vec::with_capacity(pending.len());
        for (transport_stream_id, pending) in pending.into_iter().enumerate() {
            let transport_stream_id = u64::try_from(transport_stream_id)
                .map_err(|_| "durable input transport stream id overflow".to_string())?;
            let handle = if let Some(handle) = pending.forwarded_handle {
                handle
            } else {
                let request = ProducerRegistrationRequest {
                    entity_parent_start_index: self.entity_parent_start_index,
                    coordinate: StreamRegistrationCoordinate::Root {
                        invocation_id: self.session_key.clone(),
                        root_kind: StreamRootKind::MethodInput,
                        recursive_value_path: pending.path,
                    },
                    source_invocation: self.session_key.clone(),
                    component_revision,
                    element_schema_fingerprint: pending.element_schema_fingerprint,
                    source_kind: StreamSourceKind::AgentHostedInput,
                    session_mapping: Some(session_mapping.clone()),
                };
                self.producer
                    .register(Some(context), request)
                    .await
                    .map_err(|error| error.to_string())?
                    .value
            };
            let mapping = StreamSessionMappingRecord {
                transport_stream_id,
                handle: handle.clone(),
                role: SessionStreamRole::Input,
            };
            if self.producer.owns_handle_identity(&handle) {
                self.append_mapping_once(context, mapping.clone()).await?;
            }
            self.insert_mapping(mapping.clone())?;
            mappings.push(mapping);
            if let Some(endpoint) = pending.endpoint {
                drains.push(PendingOwnedStreamDrain {
                    handle,
                    endpoint,
                    element_type: pending.element_type,
                    role: SessionStreamRole::Input,
                });
            }
        }
        if !mappings.is_empty() {
            self.commit_consumer_journal().await?;
        }

        if !drains.is_empty() {
            let streams = self.clone();
            let graph = Arc::new(graph);
            tokio::spawn(async move {
                let (nested_tx, mut nested_rx) = mpsc::unbounded_channel();
                let mut tasks = tokio::task::JoinSet::new();
                for drain in drains {
                    let streams = streams.clone();
                    let graph = graph.clone();
                    let nested_tx = nested_tx.clone();
                    tasks.spawn(async move {
                        if streams.require_root_attachment_before_production {
                            streams.wait_for_active_attachment(&drain.handle).await?;
                        }
                        streams.drain_output(drain, graph, nested_tx).await
                    });
                }
                loop {
                    // A completed parent may have queued children before its final join.
                    while let Ok(drain) = nested_rx.try_recv() {
                        let streams = streams.clone();
                        let graph = graph.clone();
                        let nested_tx = nested_tx.clone();
                        tasks.spawn(
                            async move { streams.drain_output(drain, graph, nested_tx).await },
                        );
                    }
                    if tasks.is_empty() {
                        break;
                    }
                    tokio::select! {
                        Some(drain) = nested_rx.recv() => {
                            let streams = streams.clone();
                            let graph = graph.clone();
                            let nested_tx = nested_tx.clone();
                            tasks.spawn(async move {
                                streams.drain_output(drain, graph, nested_tx).await
                            });
                        }
                        result = tasks.join_next() => {
                            match result {
                                Some(Ok(Err(error))) => {
                                    tracing::warn!(%error, "durable caller input drain failed");
                                }
                                Some(Err(error)) => {
                                    tracing::warn!(%error, "durable caller input drain task failed");
                                }
                                Some(Ok(Ok(()))) | None => {}
                            }
                        }
                    }
                }
            });
        }
        Ok((encoded, mappings))
    }

    /// Registers result streams and replaces them with session transport mappings.
    pub(crate) async fn materialize_result(
        &self,
        value: SchemaValue,
        graph: &SchemaGraph,
        root: &SchemaType,
        component_revision: golem_common::model::component::ComponentRevision,
    ) -> Result<SchemaValue, String> {
        preflight_recursive_stream_value(&value)?;
        let mut next_stream_index = 0u64;
        let retained_bytes =
            encode_recursive_stream_value_with_schema(&value, graph, root, |_stream, _path| {
                let index = next_stream_index;
                next_stream_index = next_stream_index
                    .checked_add(1)
                    .ok_or_else(|| "durable output handle index overflow".to_string())?;
                Ok(index)
            })?
            .encoded_len();
        let session = self.clone();
        let graph = graph.clone();
        let drain_graph = graph.clone();
        let root = root.clone();
        let (result, drain) = self
            .producer
            .run_admitted(
                None,
                retained_bytes,
                false,
                move |_, admission| async move {
                    let (result, drains) = session
                        .materialize_result_owned(
                            &admission,
                            value,
                            graph,
                            root,
                            component_revision,
                        )
                        .await?;
                    let drain = tokio::spawn(async move {
                        session
                            .drain_materialized_result(drains, Arc::new(drain_graph))
                            .await
                    });
                    Ok::<_, String>((result, drain))
                },
            )
            .await?;

        // Drains may live for the lifetime of the guest stream, so they must not retain the
        // producer operation permit used for the bounded durable materialization above.
        drain
            .await
            .map_err(|error| format!("durable output drain coordinator failed: {error}"))??;
        Ok(result)
    }

    async fn materialize_result_owned(
        &self,
        admission: &Arc<StreamWriteAdmission>,
        value: SchemaValue,
        graph: SchemaGraph,
        root: SchemaType,
        component_revision: golem_common::model::component::ComponentRevision,
    ) -> Result<(SchemaValue, Vec<PendingOwnedStreamDrain>), String> {
        let session_guard = self.session_lock.lock().await;
        let metadata = self.current_control_metadata().await?;
        let first_result = metadata.invocation_result.is_none();
        let cancel_session = metadata.cancellation_requested;
        let deleted_outputs = metadata
            .tombstoned_slots
            .iter()
            .filter(|(_, role)| **role == SessionStreamRole::Output)
            .map(|(slot, _)| slot.clone())
            .collect::<HashSet<_>>();
        let existing_intents = metadata
            .cancel_intents
            .keys()
            .copied()
            .collect::<HashSet<_>>();
        drop(metadata);
        let cancellation_epoch = if first_result && (cancel_session || !deleted_outputs.is_empty())
        {
            Some(self.authoritative_attachment_state().await?.0)
        } else {
            None
        };
        struct PendingOutput {
            path: Vec<StreamValuePathStep>,
            endpoint: Option<LiveStreamEndpoint>,
            forwarded_handle: Option<DurableStreamHandle>,
            element_type: SchemaType,
            element_schema_fingerprint: SchemaFingerprintV1,
            cancelled: bool,
        }

        validate_forwarded_durable_input_schemas(
            &value,
            &graph,
            &root,
            "forwarded durable output handle does not match the result stream schema",
        )?;
        let mut pending = Vec::new();
        let encoded =
            encode_recursive_stream_value_with_schema(&value, &graph, &root, |stream, path| {
                let element = stream_element_schema(&graph, &root, path)?;
                let element_schema_fingerprint =
                    schema_fingerprint_v1(&graph, element).map_err(|error| error.to_string())?;
                let forwarded = forwarded_durable_input_reference(stream)?;
                let (endpoint, forwarded_handle) = match forwarded {
                    Some(forwarded) => (None, Some(forwarded.take(stream)?.handle)),
                    None => (
                        Some(stream.take_host_endpoint::<LiveStreamEndpoint>()?),
                        None,
                    ),
                };
                let canonical_handle_index = u64::try_from(pending.len())
                    .map_err(|_| "durable output handle index overflow".to_string())?;
                let slot = match path {
                    [] => Some("$result"),
                    [StreamValuePathStep::RecordField(index)] => {
                        match graph
                            .resolve_ref(&root)
                            .map_err(|error| error.to_string())?
                        {
                            SchemaType::Record { fields, .. } => {
                                fields.get(*index as usize).map(|field| field.name.as_str())
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                };
                pending.push(PendingOutput {
                    path: path.to_vec(),
                    endpoint,
                    forwarded_handle,
                    element_type: element.cloned().unwrap_or_else(SchemaType::u8),
                    element_schema_fingerprint,
                    cancelled: first_result
                        && (cancel_session
                            || slot.is_some_and(|slot| deleted_outputs.contains(slot))),
                });
                Ok(canonical_handle_index)
            })?;

        let session_mapping = StreamSessionMapping {
            session_key: self.session_key.clone(),
            attachment_id: golem_common::model::durable_stream::AttachmentId::primary(
                self.session_key.callee_environment_id,
                &self.session_key.callee,
                &self.session_key.idempotency_key,
            )
            .map_err(|error| error.to_string())?,
            role: golem_common::model::durable_stream::SessionStreamRole::Output,
        };
        let requests = pending
            .iter()
            .filter(|pending| pending.forwarded_handle.is_none())
            .map(|pending| ProducerRegistrationRequest {
                entity_parent_start_index: self.entity_parent_start_index,
                coordinate: StreamRegistrationCoordinate::Root {
                    invocation_id: self.session_key.clone(),
                    root_kind: StreamRootKind::MethodResult,
                    recursive_value_path: pending.path.clone(),
                },
                source_kind: StreamSourceKind::InvocationOutput,
                source_invocation: self.session_key.clone(),
                component_revision,
                element_schema_fingerprint: pending.element_schema_fingerprint,
                session_mapping: Some(session_mapping.clone()),
            })
            .collect::<Vec<_>>();
        self.recover_session_mappings().await?;
        let mut new_stream_count = pending
            .iter()
            .filter(|pending| {
                pending.forwarded_handle.as_ref().is_some_and(|handle| {
                    self.mapping_for_handle(handle, SessionStreamRole::Output)
                        .is_none()
                })
            })
            .count();
        for request in &requests {
            if self
                .producer
                .handle_for_coordinate(&request.coordinate)
                .await
                .map_err(|error| error.to_string())?
                .is_none()
            {
                new_stream_count += 1;
            }
        }
        self.producer
            .validate_new_session_stream_count(&self.session_key, new_stream_count)
            .await
            .map_err(|error| error.to_string())?;
        let mut transport_stream_ids = Vec::with_capacity(pending.len());
        let mut request_index = 0usize;
        for pending in &pending {
            let transport_stream_id = if let Some(handle) = &pending.forwarded_handle {
                if pending.cancelled || existing_intents.contains(&handle.stream_id) {
                    self.mapping_for_handle(handle, SessionStreamRole::Output)
                        .map(|mapping| Ok(mapping.transport_stream_id))
                        .unwrap_or_else(|| self.allocate_transport_stream_id())?
                } else {
                    self.ensure_nested_mapping_under_lock(
                        admission,
                        handle.clone(),
                        SessionStreamRole::Output,
                    )
                    .await?
                    .transport_stream_id
                }
            } else {
                let request = &requests[request_index];
                request_index += 1;
                let existing_mapping = self
                    .producer
                    .handle_for_coordinate(&request.coordinate)
                    .await
                    .map_err(|error| error.to_string())?
                    .and_then(|handle| self.mapping_for_handle(&handle, SessionStreamRole::Output));
                existing_mapping
                    .map(|mapping| mapping.transport_stream_id)
                    .map(Ok)
                    .unwrap_or_else(|| self.allocate_transport_stream_id())?
            };
            transport_stream_ids.push(transport_stream_id);
        }
        let result_bytes = encoded.encode_to_vec();
        let mut requests = requests.into_iter();
        let outputs = pending
            .iter()
            .zip(&transport_stream_ids)
            .map(
                |(pending, &transport_stream_id)| ProducerOutputRegistration {
                    transport_stream_id,
                    cancellation_epoch: cancellation_epoch.filter(|_| {
                        pending.cancelled
                            && !pending
                                .forwarded_handle
                                .as_ref()
                                .is_some_and(|handle| existing_intents.contains(&handle.stream_id))
                    }),
                    source: match &pending.forwarded_handle {
                        Some(handle) => ProducerOutputSource::Existing(handle.clone()),
                        None => ProducerOutputSource::New(
                            requests
                                .next()
                                .expect("each owned output has a registration request"),
                        ),
                    },
                },
            )
            .collect::<Vec<_>>();
        let session_key = self.session_key.clone();
        let attribution = self.entity_parent_start_index;
        let (owned_handles, _) = admission
            .submit(move |owner, context| async move {
                let result = owner
                    .register_result_streams(
                        Some(&context),
                        session_key,
                        result_bytes,
                        outputs,
                        attribution,
                    )
                    .await?;
                owner.notify_session_records_changed(Some(&context));
                Ok::<_, StreamStoreError>(result)
            })
            .await
            .map_err(|error| error.to_string())?;

        let mut drains = Vec::with_capacity(pending.len());
        let mut owned_handles = owned_handles.into_iter();
        for (pending, transport_stream_id) in pending.into_iter().zip(transport_stream_ids) {
            let handle = pending.forwarded_handle.unwrap_or_else(|| {
                owned_handles
                    .next()
                    .expect("result registration returned too few durable handles")
            });
            self.insert_mapping(StreamSessionMappingRecord {
                transport_stream_id,
                handle: handle.clone(),
                role: SessionStreamRole::Output,
            })?;
            if let Some(endpoint) = pending.endpoint
                && !pending.cancelled
            {
                drains.push(PendingOwnedStreamDrain {
                    handle,
                    endpoint,
                    element_type: pending.element_type,
                    role: SessionStreamRole::Output,
                });
            }
        }
        drop(session_guard);

        Ok((strip_streams(value), drains))
    }

    async fn drain_materialized_result(
        &self,
        drains: Vec<PendingOwnedStreamDrain>,
        graph: Arc<SchemaGraph>,
    ) -> Result<(), String> {
        if !drains.is_empty() {
            let (nested_tx, mut nested_rx) = mpsc::unbounded_channel();
            let mut tasks = tokio::task::JoinSet::new();
            for drain in drains {
                let streams = self.clone();
                let graph = graph.clone();
                let nested_tx = nested_tx.clone();
                tasks.spawn(async move {
                    if streams.require_root_attachment_before_production {
                        streams.wait_for_active_attachment(&drain.handle).await?;
                    }
                    streams.drain_output(drain, graph, nested_tx).await
                });
            }
            loop {
                while let Ok(drain) = nested_rx.try_recv() {
                    let streams = self.clone();
                    let graph = graph.clone();
                    let nested_tx = nested_tx.clone();
                    tasks.spawn(async move { streams.drain_output(drain, graph, nested_tx).await });
                }
                if tasks.is_empty() {
                    break;
                }
                tokio::select! {
                    Some(drain) = nested_rx.recv() => {
                        let streams = self.clone();
                        let graph = graph.clone();
                        let nested_tx = nested_tx.clone();
                        tasks.spawn(async move {
                            streams.drain_output(drain, graph, nested_tx).await
                        });
                    }
                    result = tasks.join_next() => {
                        let task_result = result
                            .expect("durable output drain task set unexpectedly became empty")
                            .map_err(|error| format!("durable output drain task failed: {error}"))?;
                        task_result?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Materializes remote result mappings after validating pinned producer identities.
    pub(crate) async fn materialize_remote_result(
        &self,
        value: ProtoSchemaValue,
        remote_mappings: Vec<StreamSessionMappingRecord>,
        graph: &SchemaGraph,
        root: &SchemaType,
    ) -> Result<SchemaValue, String> {
        let transport_ids = preflight_proto_recursive_stream_value(&value)?;
        let mut by_transport = HashMap::with_capacity(remote_mappings.len());
        let mut by_handle = HashSet::with_capacity(remote_mappings.len());
        for mapping in remote_mappings {
            if by_transport
                .insert(mapping.transport_stream_id, mapping.clone())
                .is_some()
            {
                return Err(
                    "durable RPC result contains duplicate transport stream IDs".to_string()
                );
            }
            if !by_handle.insert((mapping.handle.clone(), mapping.role)) {
                return Err(
                    "durable RPC result contains duplicate durable stream handles".to_string(),
                );
            }
        }
        let referenced_transport_ids = transport_ids.iter().copied().collect::<HashSet<_>>();
        if transport_ids.len() > MAX_NEW_STREAM_HANDLES_PER_VALUE {
            return Err(
                "ResourceExhausted: durable RPC result materializes more than 256 streams"
                    .to_string(),
            );
        }
        if referenced_transport_ids.len() != transport_ids.len() {
            return Err("durable RPC result references a stream more than once".to_string());
        }
        if referenced_transport_ids != by_transport.keys().copied().collect::<HashSet<_>>() {
            return Err("durable RPC result mappings do not exactly match its value".to_string());
        }
        if by_transport
            .values()
            .any(|mapping| mapping.role != SessionStreamRole::Output)
        {
            return Err("durable RPC result stream has a non-output role".to_string());
        }
        let mut schema_transport_ids = Vec::with_capacity(transport_ids.len());
        decode_recursive_stream_value_with_schema(
            value.clone(),
            graph,
            root,
            |transport_id, path| {
                let mapping = by_transport.get(&transport_id).ok_or_else(|| {
                    format!(
                        "durable RPC result references unmapped transport stream {transport_id}"
                    )
                })?;
                let expected_fingerprint =
                    schema_fingerprint_v1(graph, stream_element_schema(graph, root, path)?)
                        .map_err(|error| error.to_string())?;
                if mapping.handle.element_schema_fingerprint != expected_fingerprint {
                    return Err(format!(
                        "durable RPC result stream {transport_id} has the wrong schema fingerprint"
                    ));
                }
                schema_transport_ids.push(transport_id);
                Ok(SchemaValueStream::from_host_endpoint(()))
            },
        )?;
        if schema_transport_ids != transport_ids {
            return Err("durable RPC result schema traversal changed stream ordering".to_string());
        }
        {
            let _session_guard = self.session_lock.lock().await;
            self.recover_session_mappings().await?;
        }
        let new_mapping_count = by_transport
            .values()
            .filter(|mapping| {
                self.mapping_for_handle(&mapping.handle, SessionStreamRole::Output)
                    .is_none()
            })
            .count();
        self.producer
            .validate_new_session_stream_count(&self.session_key, new_mapping_count)
            .await
            .map_err(|error| format!("ResourceExhausted: {error}"))?;
        let mut mappings = Vec::with_capacity(transport_ids.len());
        for transport_id in transport_ids {
            let remote = by_transport
                .get(&transport_id)
                .ok_or_else(|| {
                    format!(
                        "durable RPC result references unmapped transport stream {transport_id}"
                    )
                })?
                .clone();
            let mapping = if self.producer.owns_handle_identity(&remote.handle) {
                self.producer
                    .validate_handle(&remote.handle)
                    .await
                    .map_err(|error| error.to_string())?;
                if let Some(mapping) =
                    self.mapping_for_handle(&remote.handle, SessionStreamRole::Output)
                {
                    mapping
                } else {
                    let mapping = StreamSessionMappingRecord {
                        transport_stream_id: self.allocate_transport_stream_id()?,
                        handle: remote.handle,
                        role: SessionStreamRole::Output,
                    };
                    self.insert_mapping(mapping.clone())?;
                    mapping
                }
            } else {
                self.attach_foreign_handle(
                    remote.handle,
                    SessionStreamRole::Output,
                    self.attachment_epoch,
                )
                .await?
            };
            mappings.push(mapping);
        }
        let mut next_handle_index = 0u64;
        let canonical = remap_recursive_stream_references(value, |_, _| {
            let result = next_handle_index;
            next_handle_index = next_handle_index
                .checked_add(1)
                .ok_or_else(|| "durable result handle index overflow".to_string())?;
            Ok(result)
        })?;
        let record = StreamSessionInvocationResultRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            session_key: self.session_key.clone(),
            result: canonical.encode_to_vec(),
            output_streams: mappings
                .iter()
                .map(|mapping| mapping.handle.clone())
                .collect(),
            stream_mappings: mappings.clone(),
        };
        if let Some(existing) = self.remote_result_record().await? {
            if existing != record {
                return Err("durable RPC result conflicts with its caller journal".to_string());
            }
        } else {
            self.append_record(None, StreamSessionRecord::InvocationResult(record))
                .await;
            self.commit_consumer_journal().await?;
        }
        self.decode_initial(
            canonical,
            &mappings
                .iter()
                .map(|mapping| mapping.handle.clone())
                .collect::<Vec<_>>(),
            SessionStreamRole::Output,
        )
        .await
    }

    /// Reconstructs a previously persisted remote result without reissuing the RPC.
    pub(crate) async fn replay_remote_result(&self) -> Result<Option<SchemaValue>, String> {
        self.recover_session_mappings().await?;
        let Some(record) = self.remote_result_record().await? else {
            return Ok(None);
        };
        let value = ProtoSchemaValue::decode(record.result.as_slice())
            .map_err(|error| format!("invalid durable caller result: {error}"))?;
        self.decode_initial(value, &record.output_streams, SessionStreamRole::Output)
            .await
            .map(Some)
    }

    async fn remote_result_record(
        &self,
    ) -> Result<Option<StreamSessionInvocationResultRecord>, String> {
        let index = self.current_control_metadata().await?.invocation_result;
        let Some(index) = index else {
            return Ok(None);
        };
        match self.session_record_at(index).await? {
            StreamSessionRecord::InvocationResult(record)
                if record.session_key == self.session_key =>
            {
                Ok(Some(record))
            }
            _ => Err("durable result metadata points at a different record".into()),
        }
    }

    async fn drain_output(
        &self,
        drain: PendingOwnedStreamDrain,
        graph: Arc<SchemaGraph>,
        nested_tx: mpsc::UnboundedSender<PendingOwnedStreamDrain>,
    ) -> Result<(), String> {
        let PendingOwnedStreamDrain {
            handle,
            endpoint,
            element_type,
            role,
        } = drain;
        // Root drains require admission; children are admitted by their committed parent item.
        // A child returned unread to its producer is consumed locally, without an attachment.
        let lifecycle = endpoint.lifecycle();
        let mut source = endpoint.activate();
        let source_cancelled = tokio_util::sync::CancellationToken::new();
        let registration_id = self
            .producer
            .register_source_cancellation(handle.stream_id, source_cancelled.clone());
        let _drain_registration = OutputDrainRegistration {
            producer: self.producer.clone(),
            stream_id: handle.stream_id,
            registration_id,
            lifecycle: lifecycle.clone(),
        };
        let mut next_sequence = 0;
        let mut pending_event = None;
        loop {
            let received = match pending_event.take() {
                Some(event) => Ok(event),
                None => tokio::select! {
                    biased;
                    _ = source_cancelled.cancelled() => {
                        break;
                    },
                    _ = lifecycle.cancelled() => {
                        break;
                    },
                    received = source.recv() => received,
                },
            };
            let event = match received {
                Ok(event) => event,
                Err(LiveStreamReceiveError::Closed) if lifecycle.is_aborted() => {
                    tracing::debug!(
                        stream_id = %handle.stream_id,
                        role = ?role,
                        "Durable stream output drain closed after runtime teardown"
                    );
                    break;
                }
                Err(error) => {
                    let _ = self
                        .producer
                        .end(
                            None,
                            handle.stream_id,
                            next_sequence,
                            StreamEndResult::ErrorContext(format!("{error:?}").into_bytes()),
                        )
                        .await;
                    break;
                }
            };
            let is_item = matches!(&event.payload, LiveStreamEventPayload::Item(_));
            next_sequence = event.offset.saturating_add(1);
            let result = match event.payload {
                LiveStreamEventPayload::Item(value) => {
                    struct NestedOutput {
                        endpoint: Option<LiveStreamEndpoint>,
                        forwarded_handle: Option<DurableStreamHandle>,
                        element_type: SchemaType,
                        registration: ProducerRegistrationRequest,
                    }

                    if let Err(error) = preflight_recursive_stream_value(&value).and_then(|_| {
                        validate_forwarded_durable_input_schemas(
                            &value,
                            &graph,
                            &element_type,
                            "forwarded nested durable handle does not match the stream item schema",
                        )
                    }) {
                        let _ = self
                            .producer
                            .end(
                                None,
                                handle.stream_id,
                                event.offset,
                                StreamEndResult::ErrorContext(error.into_bytes()),
                            )
                            .await;
                        break;
                    }
                    let mut packed_u8 =
                        if matches!(graph.resolve_ref(&element_type), Ok(SchemaType::U8 { .. })) {
                            match &value {
                                SchemaValue::U8(byte) => Some(vec![*byte]),
                                _ => None,
                            }
                        } else {
                            None
                        };
                    if let Some(bytes) = packed_u8.as_mut() {
                        let flush_deadline =
                            tokio::time::Instant::now() + PACKED_U8_OUTPUT_FLUSH_DELAY;
                        while bytes.len() < MAX_PACKED_U8_STREAM_ITEM_SIZE {
                            let next = match tokio::time::timeout_at(flush_deadline, source.recv())
                                .await
                            {
                                Ok(Ok(next)) => next,
                                Ok(Err(LiveStreamReceiveError::Closed)) | Err(_) => break,
                                Ok(Err(error)) => return Err(format!("{error:?}")),
                            };
                            let expected_sequence = event
                                .offset
                                .checked_add(bytes.len() as u64)
                                .ok_or_else(|| "packed-u8 output sequence overflow".to_string())?;
                            match next.payload {
                                LiveStreamEventPayload::Item(SchemaValue::U8(byte))
                                    if next.offset == expected_sequence =>
                                {
                                    bytes.push(byte);
                                    next_sequence = next.offset.saturating_add(1);
                                }
                                payload => {
                                    pending_event =
                                        Some(crate::durable_host::stream_bus::LiveStreamEvent {
                                            offset: next.offset,
                                            payload,
                                        });
                                    break;
                                }
                            }
                        }
                    }
                    let mut nested_outputs = Vec::new();
                    let value = match encode_recursive_stream_value_with_schema(
                        &value,
                        &graph,
                        &element_type,
                        |stream, path| {
                            let nested_element =
                                stream_element_schema(&graph, &element_type, path)?.cloned();
                            let element_schema_fingerprint =
                                schema_fingerprint_v1(&graph, nested_element.as_ref())
                                    .map_err(|error| error.to_string())?;
                            let forwarded = forwarded_durable_input_reference(stream)?;
                            let (endpoint, forwarded_handle) = match forwarded {
                                Some(forwarded) => (None, Some(forwarded.take(stream)?.handle)),
                                None => (
                                    Some(stream.take_host_endpoint::<LiveStreamEndpoint>()?),
                                    None,
                                ),
                            };
                            let canonical_handle_index = u64::try_from(nested_outputs.len())
                                .map_err(|_| "durable nested handle index overflow".to_string())?;
                            nested_outputs.push(NestedOutput {
                                endpoint,
                                forwarded_handle,
                                element_type: nested_element.unwrap_or_else(SchemaType::u8),
                                registration: ProducerRegistrationRequest {
                                    entity_parent_start_index: self.entity_parent_start_index,
                                    coordinate: StreamRegistrationCoordinate::Nested {
                                        parent_stream_id: handle.stream_id,
                                        parent_producer_sequence: event.offset,
                                        recursive_value_path: path.to_vec(),
                                    },
                                    source_invocation: self.session_key.clone(),
                                    component_revision: handle.component_revision,
                                    element_schema_fingerprint,
                                    source_kind: StreamSourceKind::Nested,
                                    session_mapping: None,
                                },
                            });
                            Ok(canonical_handle_index)
                        },
                    ) {
                        Ok(value) => value,
                        Err(error) => {
                            let _ = self
                                .producer
                                .end(
                                    None,
                                    handle.stream_id,
                                    event.offset,
                                    StreamEndResult::ErrorContext(error.into_bytes()),
                                )
                                .await;
                            break;
                        }
                    };
                    for output in &nested_outputs {
                        if let Some(forwarded_handle) = &output.forwarded_handle {
                            self.ensure_nested_mapping(None, forwarded_handle.clone(), role)
                                .await?;
                        }
                    }
                    let nested_sources = nested_outputs
                        .iter()
                        .map(|output| {
                            output
                                .forwarded_handle
                                .clone()
                                .map(NestedStreamWrite::Forward)
                                .unwrap_or_else(|| {
                                    NestedStreamWrite::Register(output.registration.clone())
                                })
                        })
                        .collect();
                    let payload = match packed_u8 {
                        Some(bytes) => StreamItemsPayload::PackedU8(bytes),
                        None => StreamItemsPayload::Values(vec![value.encode_to_vec()]),
                    };
                    match self
                        .producer
                        .write_items_with_nested_sources(
                            None,
                            handle.stream_id,
                            event.offset,
                            payload,
                            nested_sources,
                        )
                        .await
                    {
                        Ok(outcome) => {
                            tracing::debug!(
                                stream_id = %handle.stream_id,
                                role = ?role,
                                first_sequence = event.offset,
                                replayed = outcome.replayed,
                                "Durable stream output items committed"
                            );
                            if !nested_outputs.is_empty() {
                                let nested_handles = self
                                    .producer
                                    .nested_handles(handle.stream_id, event.offset)
                                    .await
                                    .map_err(|error| error.to_string())?;
                                if nested_handles.len() != nested_outputs.len() {
                                    return Err("nested output stream mapping count does not match durable item metadata".to_string());
                                }
                                let memory =
                                    golem_common::serialization::serialize(&nested_handles)?.len();
                                let session = self.clone();
                                let nested_tx = nested_tx.clone();
                                self.producer
                                    .run_admitted(
                                        None,
                                        memory,
                                        false,
                                        move |_, admission| async move {
                                            let _session_guard = session.session_lock.lock().await;
                                            let session_for_write = session.clone();
                                            admission
                                                .submit(move |_, context| async move {
                                                    let session = session_for_write;
                                                    for (output, nested_handle) in nested_outputs
                                                        .into_iter()
                                                        .zip(nested_handles)
                                                    {
                                                        let transport_stream_id = session
                                                            .mapping_for_handle(
                                                                &nested_handle,
                                                                role,
                                                            )
                                                            .map(|mapping| {
                                                                mapping.transport_stream_id
                                                            })
                                                            .map(Ok)
                                                            .unwrap_or_else(|| {
                                                                session
                                                                    .allocate_transport_stream_id()
                                                            })?;
                                                        let mapping = StreamSessionMappingRecord {
                                                            transport_stream_id,
                                                            handle: nested_handle.clone(),
                                                            role,
                                                        };
                                                        session.insert_mapping(mapping.clone())?;
                                                        session
                                                            .append_mapping_once(&context, mapping)
                                                            .await?;
                                                        if let Some(endpoint) = output.endpoint {
                                                            nested_tx
                                                    .send(PendingOwnedStreamDrain {
                                                        handle: nested_handle,
                                                        endpoint,
                                                        element_type: output.element_type,
                                                        role,
                                                    })
                                                    .map_err(|_| {
                                                        "durable output drain coordinator stopped"
                                                            .to_string()
                                                    })?;
                                                        }
                                                    }
                                                    Ok::<_, String>(())
                                                })
                                                .await
                                        },
                                    )
                                    .await?;
                            }
                            Ok(())
                        }
                        Err(error) => Err(error.to_string()),
                    }
                }
                LiveStreamEventPayload::End => self
                    .producer
                    .end(None, handle.stream_id, event.offset, StreamEndResult::Ok)
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
                LiveStreamEventPayload::Error(error) => self
                    .producer
                    .end(
                        None,
                        handle.stream_id,
                        event.offset,
                        StreamEndResult::ErrorContext(error.into_bytes()),
                    )
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
            };
            if let Err(error) = result {
                let _ = self
                    .producer
                    .end_open(
                        None,
                        handle.stream_id,
                        StreamEndResult::ErrorContext(error.into_bytes()),
                    )
                    .await;
                break;
            }
            if !is_item {
                break;
            }
        }
        Ok(())
    }

    async fn wait_for_active_attachment(&self, handle: &DurableStreamHandle) -> Result<(), String> {
        loop {
            let changed = self.producer.session_records_changed().notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            // A terminal output only reconstructs committed events. Its consumer may
            // already have detached permanently, so replay cannot require an attachment.
            if self
                .producer
                .input_high_water(handle.stream_id)
                .await
                .map_err(|error| error.to_string())?
                .is_some_and(|high_water| high_water.terminal)
            {
                return Ok(());
            }
            let active = self
                .producer
                .has_active_attachment(&self.session_key, handle)
                .await
                .map_err(|error| error.to_string())?;
            if active {
                return Ok(());
            }
            changed.await;
        }
    }

    async fn finish(
        &self,
        result: Result<(), Vec<u8>>,
        input_cancel_reason: StreamCancelReason,
    ) -> Result<(), String> {
        if self.has_committed_finished().await? {
            return Ok(());
        }
        let session = self.clone();
        let retained_bytes = DurableStreamStore::finish_session_retained_bytes(&result);
        let outcome = self
            .producer
            .run_admitted(None, retained_bytes, true, move |_, admission| async move {
                let _session_guard = session.session_lock.lock().await;
                session.validate_topology_complete().await?;
                let session_for_write = session.clone();
                admission
                    .submit(move |owner, context| async move {
                        owner
                            .finish_session(
                                Some(&context),
                                session_for_write.session_key.clone(),
                                session_for_write.entity_parent_start_index,
                                result,
                                input_cancel_reason,
                            )
                            .await
                            .map_err(|error| error.to_string())
                    })
                    .await
            })
            .await;
        match outcome {
            Ok(()) => Ok(()),
            Err(error) => match self.has_committed_finished().await {
                Ok(true) => Ok(()),
                Ok(false) => Err(error),
                Err(lookup_error) => Err(format!(
                    "{error}; committed finish lookup failed: {lookup_error}"
                )),
            },
        }
    }

    async fn has_committed_finished(&self) -> Result<bool, String> {
        let Some(journal) = &self.consumer_journal else {
            return Ok(false);
        };
        let Some(index) = journal.committed_finished_index(&self.session_key).await? else {
            return Ok(false);
        };
        match self.session_record_at(index).await? {
            StreamSessionRecord::Finished(record) if record.session_key == self.session_key => {
                Ok(true)
            }
            _ => Err("committed finished metadata points at a different session record".into()),
        }
    }

    async fn validate_topology_complete(&self) -> Result<(), String> {
        let metadata = self.current_control_metadata().await?;
        if let Some(error) = &metadata.topology_error {
            return Err(error.clone());
        }
        for topology in metadata.topologies.values() {
            if !topology.is_active() {
                return Err(
                    "durable session has prepared but inactive foreign topology".to_string()
                );
            }
            let mapping = &topology.mapping;
            if !metadata.visible_mappings.contains(&(
                mapping.transport_stream_id,
                mapping.handle.clone(),
                mapping.role,
            )) {
                return Err(
                    "durable session has activated foreign topology without a visible mapping"
                        .to_string(),
                );
            }
        }
        Ok(())
    }

    /// Records a failed session terminal after cancelling remaining stream work.
    pub(crate) async fn fail(&self, details: String) -> Result<(), String> {
        self.finish(
            Err(details.into_bytes()),
            StreamCancelReason::InvocationFailed,
        )
        .await
    }

    /// Finalizes streams with invocation-failed semantics and records session failure.
    pub(crate) async fn fail_invocation(&self, details: String) -> Result<(), String> {
        self.finish(
            Err(details.into_bytes()),
            StreamCancelReason::InvocationFailed,
        )
        .await
    }

    /// Finalizes streams with protocol-error semantics and records session failure.
    pub(crate) async fn fail_protocol(&self, details: String) -> Result<(), String> {
        self.finish(Err(details.into_bytes()), StreamCancelReason::Protocol)
            .await
    }

    /// Records successful session completion once all protocol terminals are durable.
    pub(crate) async fn complete(&self) -> Result<(), String> {
        self.finish(Ok(()), StreamCancelReason::GuestDrop).await
    }

    /// Completes now unless forwarded inputs still require later terminal processing.
    pub(crate) async fn complete_or_defer_for_forwarded_inputs(&self) -> Result<(), String> {
        if self
            .producer
            .has_open_forwarded_session_input(&self.session_key)
            .await
            .map_err(|error| error.to_string())?
        {
            Ok(())
        } else {
            self.complete().await
        }
    }

    /// Reads the committed invocation result, independently of stream drain progress.
    pub(crate) async fn persisted_result(
        &self,
    ) -> Result<Option<(ProtoSchemaValue, Vec<DurableStreamMapping>)>, String> {
        if let Some(result) = self.remote_result_record().await? {
            let value = ProtoSchemaValue::decode(result.result.as_slice())
                .map_err(|error| format!("invalid persisted durable invocation result: {error}"))?;
            let transport_value = remap_recursive_stream_references(value, |handle_index, _| {
                let index = usize::try_from(handle_index).map_err(|_| {
                    format!("durable result handle index {handle_index} is too large")
                })?;
                result
                    .stream_mappings
                    .get(index)
                    .map(|mapping| mapping.transport_stream_id)
                    .ok_or_else(|| format!("unknown durable result handle index {handle_index}"))
            })?;
            let proto_mappings = result
                .stream_mappings
                .iter()
                .map(|mapping| durable_stream_mapping_to_proto(mapping, None))
                .collect();
            for mapping in result.stream_mappings {
                self.insert_mapping(mapping)?;
            }
            return Ok(Some((transport_value, proto_mappings)));
        }
        Ok(None)
    }

    /// Waits for a committed invocation result or session failure.
    pub(crate) async fn wait_persisted_result(
        &self,
    ) -> Result<(ProtoSchemaValue, Vec<DurableStreamMapping>), String> {
        loop {
            let changed = self.producer.session_records_changed().notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some(result) = self.persisted_result().await? {
                return Ok(result);
            }
            changed.await;
        }
    }

    /// Reads the committed session terminal without waiting.
    pub(crate) async fn persisted_finished(&self) -> Result<Option<Result<(), Vec<u8>>>, String> {
        let index = self.current_control_metadata().await?.finished;
        let Some(index) = index else {
            return Ok(None);
        };
        match self.session_record_at(index).await? {
            StreamSessionRecord::Finished(record) if record.session_key == self.session_key => {
                Ok(Some(record.result))
            }
            _ => Err("durable finished metadata points at a different record".into()),
        }
    }

    /// Waits until successful or failed session completion is durable.
    pub(crate) async fn wait_persisted_finished(&self) -> Result<Result<(), Vec<u8>>, String> {
        loop {
            let changed = self.producer.session_records_changed().notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some(result) = self.persisted_finished().await? {
                return Ok(result);
            }
            changed.await;
        }
    }

    /// Drives output streams from committed producer history into transport callbacks.
    pub(crate) async fn pump_output_streams(
        &self,
        responses: &mpsc::Sender<InvocationResponse>,
    ) -> Result<(), String> {
        self.recover_session_mappings().await?;
        let root_output_mapping_ids = self.session_root_output_mapping_ids().await?;
        self.pump_output_streams_from_recovered(
            &HashMap::new(),
            &root_output_mapping_ids,
            &[],
            responses,
        )
        .await
    }

    /// Replays output streams for a resumed attachment.
    ///
    /// The root output streams are pumped first, and nested streams are pumped only after the
    /// enclosing item that announces their transport mapping has been emitted, so the client
    /// observes the same tree order as during the original invocation. Output streams that were
    /// already mapped but are not reached by that traversal, because a cursor skipped the parent
    /// item that introduced them, are pumped afterwards.
    pub(crate) async fn pump_output_streams_from(
        &self,
        cursors: &HashMap<
            golem_common::model::durable_stream::StreamId,
            Option<golem_common::model::durable_stream::StreamOffset>,
        >,
        root_output_mapping_ids: &[u64],
        known_output_mapping_ids: &[u64],
        responses: &mpsc::Sender<InvocationResponse>,
    ) -> Result<(), String> {
        self.recover_session_mappings().await?;
        self.pump_output_streams_from_recovered(
            cursors,
            root_output_mapping_ids,
            known_output_mapping_ids,
            responses,
        )
        .await
    }

    async fn pump_output_streams_from_recovered(
        &self,
        cursors: &HashMap<
            golem_common::model::durable_stream::StreamId,
            Option<golem_common::model::durable_stream::StreamOffset>,
        >,
        root_output_mapping_ids: &[u64],
        known_output_mapping_ids: &[u64],
        responses: &mpsc::Sender<InvocationResponse>,
    ) -> Result<(), String> {
        let mut seen = HashSet::new();
        self.pump_output_stream_trees(cursors, root_output_mapping_ids, &mut seen, responses)
            .await?;
        let detached = known_output_mapping_ids
            .iter()
            .copied()
            .filter(|transport_stream_id| !seen.contains(transport_stream_id))
            .collect::<Vec<_>>();
        self.pump_output_stream_trees(cursors, &detached, &mut seen, responses)
            .await
    }

    async fn pump_output_stream_trees(
        &self,
        cursors: &HashMap<
            golem_common::model::durable_stream::StreamId,
            Option<golem_common::model::durable_stream::StreamOffset>,
        >,
        output_mapping_ids: &[u64],
        seen: &mut HashSet<u64>,
        responses: &mpsc::Sender<InvocationResponse>,
    ) -> Result<(), String> {
        let mut pending = output_mapping_ids
            .iter()
            .copied()
            .filter(|transport_stream_id| seen.insert(*transport_stream_id))
            .map(|transport_stream_id| {
                let mapping = self.mapping(transport_stream_id).ok_or_else(|| {
                    format!("unknown durable output stream {transport_stream_id}")
                })?;
                Ok((transport_stream_id, mapping.handle))
            })
            .collect::<Result<Vec<_>, String>>()?;
        while !pending.is_empty() {
            let nested = try_join_all(pending.into_iter().map(|(transport_stream_id, handle)| {
                let after = cursors.get(&handle.stream_id).copied().flatten();
                self.pump_output_stream_from(transport_stream_id, handle, after, responses)
            }))
            .await?;
            pending = nested
                .into_iter()
                .flatten()
                .filter(|(transport_stream_id, _)| seen.insert(*transport_stream_id))
                .collect();
        }
        Ok(())
    }

    /// Sends the guest-authored cancellation of every callee-owned input stream to the attached
    /// client. Inputs whose acceptance already announced a terminal high water are skipped: the
    /// client learned about that cancellation from the acceptance, and re-sending the same
    /// durable offset would violate the strictly increasing offset rule of the session protocol.
    pub(crate) async fn pump_input_cancellations(
        &self,
        responses: &mpsc::Sender<InvocationResponse>,
        announced_high_waters: &HashMap<u64, InputStreamHighWater>,
    ) -> Result<(), String> {
        self.recover_session_mappings().await?;
        let inputs = self
            .mappings
            .read()
            .expect("durable stream mapping lock poisoned")
            .iter()
            .filter_map(|(transport_stream_id, mapping)| {
                (mapping.role == SessionStreamRole::Input
                    && mapping.handle.producer_environment_id
                        == self.session_key.callee_environment_id
                    && mapping.handle.producer == self.session_key.callee
                    && mapping.handle.expected_producer_fingerprint
                        == self.session_key.callee_fingerprint
                    && !announced_high_waters
                        .get(transport_stream_id)
                        .is_some_and(|high_water| high_water.terminal))
                .then_some((*transport_stream_id, mapping.handle.clone()))
            })
            .collect::<Vec<_>>();
        for (transport_stream_id, handle) in inputs {
            let mut reader = self
                .producer
                .catch_up(handle.clone(), None)
                .await
                .map_err(|error| error.to_string())?;
            while let Some(event) = reader.next().await.map_err(|error| error.to_string())? {
                match event.payload {
                    CommittedProducerStreamEventPayload::Cancel {
                        role: StreamCancelRole::InputConsumer,
                        reason,
                        details,
                    } => {
                        responses
                            .send(InvocationResponse {
                                response: Some(invocation_response::Response::StreamCancel(
                                    StreamCancel {
                                        transport_stream_id,
                                        producer_sequence: event.producer_sequence,
                                        role: golem_api_grpc::proto::golem::worker::StreamCancelRole::InputConsumer as i32,
                                        reason: stream_cancel_reason_to_proto(reason) as i32,
                                        details,
                                        durable_stream_id: Some(handle.stream_id.0.into()),
                                        epoch: self.attachment_epoch,
                                        durable_offset: event.offset.0.to_vec(),
                                    },
                                )),
                            })
                            .await
                            .map_err(|_| "invocation response stream closed".to_string())?;
                        break;
                    }
                    CommittedProducerStreamEventPayload::End(_)
                    | CommittedProducerStreamEventPayload::Cancel { .. } => break,
                    CommittedProducerStreamEventPayload::Value(_)
                    | CommittedProducerStreamEventPayload::PackedU8(_) => {}
                }
            }
        }
        Ok(())
    }

    async fn pump_output_stream_from(
        &self,
        transport_stream_id: u64,
        handle: DurableStreamHandle,
        after: Option<golem_common::model::durable_stream::StreamOffset>,
        responses: &mpsc::Sender<InvocationResponse>,
    ) -> Result<Vec<(u64, DurableStreamHandle)>, String> {
        let durable_stream_id = handle.stream_id;
        let (mut nested_streams, terminal_cursor) = if let Some(through) = after {
            self.recover_session_mappings().await?;
            self.output_mappings_introduced_through(&handle, through)
                .await?
        } else {
            (Vec::new(), false)
        };
        if terminal_cursor {
            return Ok(nested_streams);
        }
        let mut reader = self
            .stream_reader(handle, after, SessionStreamRole::Output)
            .await?;
        let mut pending_event = None;
        loop {
            let event = match pending_event.take() {
                Some(event) => event,
                None => match reader.next().await.map_err(|error| error.to_string())? {
                    Some(event) => event,
                    None => break,
                },
            };
            self.ensure_current_attachment().await?;
            let response = match event.payload {
                CommittedProducerStreamEventPayload::Value(bytes) => {
                    let memory = bytes.len()
                        + golem_common::serialization::serialize(&event.nested_handles)?.len();
                    let session = self.clone();
                    let (response, introduced) = self
                        .producer
                        .run_admitted(None, memory, false, move |_, admission| async move {
                            let _session_guard = session.session_lock.lock().await;
                            let session_for_write = session.clone();
                            admission.submit(move |_: Arc<DurableStreamStore>, context: StreamWriteContext| async move {
                            let session = session_for_write;
                            session.ensure_current_attachment().await?;
                            session.recover_session_mappings().await?;
                            let mut nested_streams = Vec::new();
                            let value =
                                ProtoSchemaValue::decode(bytes.as_slice()).map_err(|error| {
                                    format!("invalid durable output value: {error}")
                                })?;
                            let handle_indices = preflight_proto_recursive_stream_value(&value)?;
                            let nested_handles = event.nested_handles.clone();
                            if nested_handles.len() != handle_indices.len() {
                                return Err(
                                    "nested output handle count does not match the canonical value"
                                        .to_string(),
                                );
                            }
                            let mut mappings = Vec::with_capacity(nested_handles.len());
                            for (position, (handle_index, handle)) in
                                handle_indices.into_iter().zip(nested_handles).enumerate()
                            {
                                if handle_index != position as u64 {
                                    return Err(format!(
                                        "invalid canonical nested output handle index {handle_index}"
                                    ));
                                }
                                let mapping = match session
                                    .mapping_for_handle(&handle, SessionStreamRole::Output)
                                {
                                    Some(mapping) => mapping,
                                    None => {
                                        let transport_stream_id =
                                            session.allocate_transport_stream_id()?;
                                        let mapping = StreamSessionMappingRecord {
                                            transport_stream_id,
                                            handle: handle.clone(),
                                            role: SessionStreamRole::Output,
                                        };
                                        session.insert_mapping(mapping.clone())?;
                                        session
                                            .append_mapping_once(&context, mapping.clone())
                                            .await?;
                                        mapping
                                    }
                                };
                                nested_streams
                                    .push((mapping.transport_stream_id, mapping.handle.clone()));
                                mappings.push(mapping);
                            }
                            if !mappings.is_empty() {
                                session.commit_consumer_journal().await?;
                            }
                            let value =
                                remap_recursive_stream_references(value, |handle_index, _| {
                                    let index = usize::try_from(handle_index).map_err(|_| {
                            format!(
                                "durable nested output handle index {handle_index} is too large"
                            )
                        })?;
                                    mappings
                            .get(index)
                            .map(|mapping| mapping.transport_stream_id)
                            .ok_or_else(|| {
                                format!("unknown durable nested output handle index {handle_index}")
                            })
                                })?;
                            let new_stream_mappings = mappings
                                .iter()
                                .map(|mapping| durable_stream_mapping_to_proto(mapping, None))
                                .collect();
                            let response =
                                invocation_response::Response::OutputItem(OutputStreamItem {
                                    transport_stream_id,
                                    producer_sequence: event.producer_sequence,
                                    value: Some(value),
                                    durable_stream_id: Some(durable_stream_id.0.into()),
                                    durable_offset: event.offset.0.to_vec(),
                                    epoch: session.attachment_epoch,
                                    new_stream_mappings,
                                    packed_u8: Vec::new(),
                                    logical_item_count: 1,
                                });
                            Ok::<_, String>((response, nested_streams))
                            }).await
                        })
                        .await?;
                    nested_streams.extend(introduced);
                    response
                }
                CommittedProducerStreamEventPayload::PackedU8(value) => {
                    if !event.nested_handles.is_empty() {
                        return Err(
                            "packed-u8 durable output item contains nested stream handles"
                                .to_string(),
                        );
                    }
                    let first_sequence = event.producer_sequence;
                    let mut final_offset = event.offset;
                    let mut bytes = Vec::with_capacity(1024);
                    bytes.push(value);
                    let flush_deadline = tokio::time::Instant::now() + PACKED_U8_OUTPUT_FLUSH_DELAY;
                    while bytes.len() < MAX_PACKED_U8_STREAM_ITEM_SIZE {
                        let next =
                            match tokio::time::timeout_at(flush_deadline, reader.next()).await {
                                Ok(Ok(Some(next))) => next,
                                Ok(Ok(None)) | Err(_) => break,
                                Ok(Err(error)) => return Err(error.to_string()),
                            };
                        let expected_sequence = first_sequence
                            .checked_add(bytes.len() as u64)
                            .ok_or_else(|| "packed-u8 output sequence overflow".to_string())?;
                        match &next.payload {
                            CommittedProducerStreamEventPayload::PackedU8(byte)
                                if next.producer_sequence == expected_sequence
                                    && next.nested_handles.is_empty() =>
                            {
                                bytes.push(*byte);
                                final_offset = next.offset;
                            }
                            _ => {
                                pending_event = Some(next);
                                break;
                            }
                        }
                    }
                    self.ensure_current_attachment().await?;
                    invocation_response::Response::OutputItem(OutputStreamItem {
                        transport_stream_id,
                        producer_sequence: first_sequence,
                        value: None,
                        durable_stream_id: Some(durable_stream_id.0.into()),
                        durable_offset: final_offset.0.to_vec(),
                        epoch: self.attachment_epoch,
                        new_stream_mappings: Vec::new(),
                        logical_item_count: bytes.len() as u64,
                        packed_u8: bytes,
                    })
                }
                CommittedProducerStreamEventPayload::End(StreamEndResult::Ok) => {
                    invocation_response::Response::OutputEnd(OutputStreamEnd {
                        transport_stream_id,
                        producer_sequence: event.producer_sequence,
                        durable_stream_id: Some(durable_stream_id.0.into()),
                        durable_offset: event.offset.0.to_vec(),
                        epoch: self.attachment_epoch,
                    })
                }
                CommittedProducerStreamEventPayload::End(StreamEndResult::ErrorContext(
                    details,
                )) => invocation_response::Response::OutputError(OutputStreamError {
                    transport_stream_id,
                    producer_sequence: event.producer_sequence,
                    details: String::from_utf8_lossy(&details).into_owned(),
                    durable_stream_id: Some(durable_stream_id.0.into()),
                    durable_offset: event.offset.0.to_vec(),
                    epoch: self.attachment_epoch,
                }),
                CommittedProducerStreamEventPayload::Cancel {
                    role: _,
                    reason,
                    details,
                } => invocation_response::Response::StreamCancel(StreamCancel {
                    transport_stream_id,
                    producer_sequence: event.producer_sequence,
                    role: golem_api_grpc::proto::golem::worker::StreamCancelRole::OutputProducer
                        as i32,
                    reason: stream_cancel_reason_to_proto(reason) as i32,
                    details,
                    durable_stream_id: Some(durable_stream_id.0.into()),
                    epoch: self.attachment_epoch,
                    durable_offset: event.offset.0.to_vec(),
                }),
            };
            let terminal = matches!(
                response,
                invocation_response::Response::OutputEnd(_)
                    | invocation_response::Response::OutputError(_)
                    | invocation_response::Response::StreamCancel(_)
            );
            responses
                .send(InvocationResponse {
                    response: Some(response),
                })
                .await
                .map_err(|_| "invocation response stream closed".to_string())?;
            if terminal {
                break;
            }
        }
        Ok(nested_streams)
    }

    async fn output_mappings_introduced_through(
        &self,
        handle: &DurableStreamHandle,
        through: golem_common::model::durable_stream::StreamOffset,
    ) -> Result<(Vec<(u64, DurableStreamHandle)>, bool), String> {
        let mut after = None;
        let mut nested_streams = Vec::new();
        let mut seen = HashSet::new();
        loop {
            let events = if self.producer.owns_handle_identity(handle) {
                self.producer
                    .read_segment(handle, after, Some(through))
                    .await
                    .map_err(|error| error.to_string())?
            } else {
                let mapping = self
                    .mapping_for_handle(handle, SessionStreamRole::Output)
                    .ok_or_else(|| "foreign durable stream has no session mapping".to_string())?;
                let attachment = self.attachment_key(handle, self.attachment_epoch)?;
                if self.topology_state(&attachment, Some(&mapping)).await?
                    != ConsumerAttachmentStatus::Active
                {
                    return Err(
                        "foreign durable stream mapping is not topology-activated".to_string()
                    );
                }
                let rpc = self.rpc.clone().ok_or_else(|| {
                    "foreign durable stream source routing is unavailable".to_string()
                })?;
                let auth_ctx = self.auth_ctx.clone().ok_or_else(|| {
                    "foreign durable stream consumer authorization is unavailable".to_string()
                })?;
                RoutedAttachedStreamSegmentSource::new(
                    rpc,
                    mapping,
                    auth_ctx,
                    self.producer.clone(),
                )
                .read_attached_segment(
                    &attachment,
                    handle,
                    Timestamp::now_utc().to_millis(),
                    after,
                    Some(through),
                )
                .await
                .map_err(|error| error.to_string())?
            };
            let last = events
                .last()
                .ok_or_else(|| "stream replay ended before its requested cursor".to_string())?;
            if after.is_some_and(|after| last.offset <= after) {
                return Err("stream replay page did not advance its cursor".into());
            }
            after = Some(last.offset);
            let terminal_cursor = last.is_terminal();
            let page = events
                .into_iter()
                .flat_map(|event| event.nested_handles)
                .filter(|handle| seen.insert(handle.stream_id))
                .map(|nested_handle| {
                    let mapping = self
                        .mapping_for_handle(&nested_handle, SessionStreamRole::Output)
                        .ok_or_else(|| {
                            format!(
                                "durable nested output stream {} has no session mapping",
                                nested_handle.stream_id
                            )
                        })?;
                    Ok((mapping.transport_stream_id, nested_handle))
                })
                .collect::<Result<Vec<_>, String>>()?;
            nested_streams.extend(page);
            if after == Some(through) {
                return Ok((nested_streams, terminal_cursor));
            }
        }
    }

    /// Returns output IDs whose terminal cursors can be reconstructed from committed records.
    pub(crate) async fn terminal_output_cursor_stream_ids(
        &self,
        cursors: &HashMap<
            golem_common::model::durable_stream::StreamId,
            Option<golem_common::model::durable_stream::StreamOffset>,
        >,
    ) -> Result<HashSet<golem_common::model::durable_stream::StreamId>, String> {
        self.recover_session_mappings().await?;
        let candidates = self
            .mappings
            .read()
            .expect("durable stream mapping lock poisoned")
            .values()
            .filter_map(|mapping| {
                if mapping.role != SessionStreamRole::Output {
                    return None;
                }
                cursors
                    .get(&mapping.handle.stream_id)
                    .copied()
                    .flatten()
                    .map(|cursor| (mapping.handle.clone(), cursor))
            })
            .collect::<Vec<_>>();
        let mut terminal = HashSet::new();
        for (handle, cursor) in candidates {
            let (_, terminal_cursor) = self
                .output_mappings_introduced_through(&handle, cursor)
                .await?;
            if terminal_cursor {
                terminal.insert(handle.stream_id);
            }
        }
        Ok(terminal)
    }

    /// Returns transport IDs durably classified as root outputs.
    pub(crate) async fn session_root_output_mapping_ids(&self) -> Result<Vec<u64>, String> {
        Ok(self.current_control_metadata().await?.root_outputs.clone())
    }

    /// Decodes initial transport input while preserving durable stream placeholders.
    pub(crate) async fn decode_initial(
        &self,
        value: ProtoSchemaValue,
        handles: &[DurableStreamHandle],
        role: SessionStreamRole,
    ) -> Result<SchemaValue, String> {
        let ids = preflight_proto_recursive_stream_value(&value)?;
        let mut endpoints = HashMap::with_capacity(ids.len());
        for handle_index in ids {
            let index = usize::try_from(handle_index)
                .map_err(|_| format!("durable input handle index {handle_index} is too large"))?;
            let handle = handles
                .get(index)
                .cloned()
                .ok_or_else(|| format!("unknown durable input handle index {handle_index}"))?;
            endpoints.insert(handle_index, self.endpoint(handle, 0, role).await?);
        }
        decode_recursive_stream_value(value, |handle_index, _| {
            endpoints
                .remove(&handle_index)
                .map(SchemaValueStream::from_host_endpoint)
                .ok_or_else(|| {
                    format!("duplicate or unknown durable input handle index {handle_index}")
                })
        })
    }

    async fn endpoint(
        &self,
        handle: DurableStreamHandle,
        consumer_read_ordinal: u64,
        role: SessionStreamRole,
    ) -> Result<DurableInputEndpoint, String> {
        let (journal, after, _, terminal) = self.consumer_history(handle.stream_id).await?;
        let mut reader = if terminal {
            None
        } else {
            Some(self.stream_reader(handle.clone(), after, role).await?)
        };
        record_source_journal_lag(reader.as_mut(), after, true).await;
        Ok(DurableInputEndpoint {
            reader,
            journal,
            streams: self.clone(),
            transport_stream_id: self
                .mapping_for_handle(&handle, role)
                .ok_or_else(|| "durable input endpoint has no session mapping".to_string())?
                .transport_stream_id,
            handle,
            consumer_read_ordinal,
            role,
        })
    }

    async fn stream_reader(
        &self,
        handle: DurableStreamHandle,
        after: Option<golem_common::model::durable_stream::StreamOffset>,
        role: SessionStreamRole,
    ) -> Result<DurableStreamReader, String> {
        let reader = if self.producer.owns_handle_identity(&handle) {
            DurableStreamReader::Owned {
                reader: Box::new(
                    self.producer
                        .catch_up(handle.clone(), after)
                        .await
                        .map_err(|error| error.to_string())?,
                ),
                source: self.producer.clone(),
                handle: Box::new(handle),
                next_journal_lag_sample: Instant::now(),
            }
        } else {
            let mapping = self
                .mapping_for_handle(&handle, role)
                .ok_or_else(|| "foreign durable stream has no session mapping".to_string())?;
            let attachment = self.attachment_key(&handle, self.attachment_epoch)?;
            if self.topology_state(&attachment, Some(&mapping)).await?
                != ConsumerAttachmentStatus::Active
            {
                return Err("foreign durable stream mapping is not topology-activated".to_string());
            }
            let rpc = self.rpc.clone().ok_or_else(|| {
                "foreign durable stream source routing is unavailable".to_string()
            })?;
            let auth_ctx = self.auth_ctx.clone().ok_or_else(|| {
                "foreign durable stream consumer authorization is unavailable".to_string()
            })?;
            let consumer_producer = self
                .consumer_journal
                .as_ref()
                .map(|_| self.producer.clone());
            DurableStreamReader::Attached(Box::new(AttachedDurableCatchUpReader {
                source: Arc::new(RoutedAttachedStreamSegmentSource::new(
                    rpc,
                    mapping,
                    auth_ctx,
                    self.producer.clone(),
                )),
                attachment,
                handle: handle.clone(),
                consumer_producer,
                after,
                buffered: VecDeque::new(),
                terminal: false,
                next_journal_lag_sample: Instant::now(),
            }))
        };
        Ok(reader)
    }

    async fn consumer_history_positions(
        &self,
        stream_id: golem_common::model::StreamId,
    ) -> Result<Vec<OplogIndex>, String> {
        let (mut covered, mut positions) = self
            .producer
            .persisted_consumer_positions(&self.session_key, stream_id)
            .await?
            .unwrap_or((OplogIndex::NONE, Vec::new()));
        let horizon = self.oplog.current_oplog_index().await;
        while covered < horizon {
            let count = (horizon.as_u64() - covered.as_u64()).min(1024);
            for (index, entry) in self.oplog.read_exact(covered.next(), count).await {
                if let OplogEntry::StreamSession { record, .. } = entry {
                    let record = self.download_record(record).await?;
                    let matches = match record {
                        StreamSessionRecord::ConsumerItemValue(record) => {
                            record.session_key == self.session_key && record.stream_id == stream_id
                        }
                        StreamSessionRecord::ConsumerTerminal(record) => {
                            record.session_key == self.session_key && record.stream_id == stream_id
                        }
                        StreamSessionRecord::SourceUnavailable(record) => {
                            record.key.session_key == self.session_key
                                && record.key.stream_id == stream_id
                        }
                        _ => false,
                    };
                    if matches {
                        positions.push(index);
                    }
                }
                covered = index;
            }
        }
        Ok(positions)
    }

    async fn consumer_history(
        &self,
        stream_id: golem_common::model::durable_stream::StreamId,
    ) -> Result<
        (
            VecDeque<CommittedProducerStreamEvent>,
            Option<golem_common::model::durable_stream::StreamOffset>,
            u64,
            bool,
        ),
        String,
    > {
        let mut events = Vec::new();
        for index in self.consumer_history_positions(stream_id).await? {
            let record = self.session_record_at(index).await?;
            match record {
                StreamSessionRecord::ConsumerItemValue(record)
                    if record.session_key == self.session_key && record.stream_id == stream_id =>
                {
                    for mapping in &record.recursive_mappings {
                        self.insert_mapping(mapping.clone())?;
                    }
                    if record.packed_u8 {
                        if !record.recursive_handles.is_empty() {
                            return Err(
                                "packed-u8 consumer journal contains nested stream handles"
                                    .to_string(),
                            );
                        }
                        let batch_end = record
                            .source_offset_at(record.logical_item_count().saturating_sub(1))
                            .ok_or_else(|| {
                                "packed-u8 consumer journal offset range is invalid".to_string()
                            })?;
                        for (index, byte) in record.value.iter().copied().enumerate() {
                            let ordinal = record
                                .consumer_read_ordinal
                                .checked_add(index as u64)
                                .ok_or_else(|| {
                                    "packed-u8 consumer journal ordinal overflow".to_string()
                                })?;
                            events.push((
                                ordinal,
                                CommittedProducerStreamEvent {
                                    stream_id,
                                    producer_sequence: ordinal,
                                    offset: record.source_offset_at(index).ok_or_else(|| {
                                        "packed-u8 consumer journal offset range is invalid"
                                            .to_string()
                                    })?,
                                    packed_u8_batch_end: Some(batch_end),
                                    terminal_author: None,
                                    nested_handles: Vec::new(),
                                    payload: CommittedProducerStreamEventPayload::PackedU8(byte),
                                },
                            ));
                        }
                    } else {
                        events.push((
                            record.consumer_read_ordinal,
                            CommittedProducerStreamEvent {
                                stream_id,
                                producer_sequence: record.consumer_read_ordinal,
                                offset: record.source_offset,
                                packed_u8_batch_end: None,
                                terminal_author: None,
                                nested_handles: record.recursive_handles,
                                payload: CommittedProducerStreamEventPayload::Value(record.value),
                            },
                        ));
                    }
                }
                StreamSessionRecord::ConsumerTerminal(record)
                    if record.session_key == self.session_key && record.stream_id == stream_id =>
                {
                    events.push((
                        record.consumer_read_ordinal,
                        CommittedProducerStreamEvent {
                            stream_id,
                            producer_sequence: record.consumer_read_ordinal,
                            offset: record.source_offset,
                            packed_u8_batch_end: None,
                            terminal_author: None,
                            nested_handles: Vec::new(),
                            payload: match record.terminal {
                                StreamConsumerTerminal::End(result) => {
                                    CommittedProducerStreamEventPayload::End(result)
                                }
                                StreamConsumerTerminal::Cancel {
                                    role,
                                    reason,
                                    details,
                                } => CommittedProducerStreamEventPayload::Cancel {
                                    role,
                                    reason,
                                    details,
                                },
                            },
                        },
                    ));
                }
                StreamSessionRecord::SourceUnavailable(record)
                    if record.key.session_key == self.session_key
                        && record.key.stream_id == stream_id =>
                {
                    events.push((
                        record.consumer_read_ordinal,
                        CommittedProducerStreamEvent {
                            stream_id,
                            producer_sequence: record.consumer_read_ordinal,
                            offset: record.source_offset,
                            packed_u8_batch_end: None,
                            terminal_author: None,
                            nested_handles: Vec::new(),
                            payload: CommittedProducerStreamEventPayload::Cancel {
                                role: StreamCancelRole::System,
                                reason: StreamCancelReason::SourceUnavailable,
                                details: None,
                            },
                        },
                    ));
                }
                _ => {}
            }
        }
        events.sort_by_key(|(ordinal, _)| *ordinal);
        for (expected, (ordinal, _)) in events.iter().enumerate() {
            if *ordinal != expected as u64 {
                return Err("consumer value journal contains a read-ordinal gap".to_string());
            }
        }
        let after = events.last().map(|(_, event)| event.offset);
        let next_ordinal = events.len() as u64;
        let terminal = events.last().is_some_and(|(_, event)| event.is_terminal());
        Ok((
            events.into_iter().map(|(_, event)| event).collect(),
            after,
            next_ordinal,
            terminal,
        ))
    }
}

#[async_trait::async_trait]
impl StreamAttachmentConsumerProbe for StreamSession {
    async fn status(
        &self,
        key: &StreamAttachmentKey,
    ) -> Result<ConsumerAttachmentStatus, StreamStoreError> {
        self.topology_state(key, None)
            .await
            .map_err(StreamStoreError::Oplog)
    }
}

/// Session endpoint used by a guest-facing durable input consumer.
pub(crate) struct DurableInputEndpoint {
    reader: Option<DurableStreamReader>,
    journal: VecDeque<CommittedProducerStreamEvent>,
    streams: StreamSession,
    transport_stream_id: u64,
    handle: DurableStreamHandle,
    consumer_read_ordinal: u64,
    role: SessionStreamRole,
}

/// Foreign source and attachment state for a forwarded durable input.
pub(crate) struct ForwardedDurableInput {
    pub(crate) handle: DurableStreamHandle,
}

impl DurableInputEndpoint {
    fn forwarded_handle(&self) -> Result<DurableStreamHandle, String> {
        if self.consumer_read_ordinal != 0 || !self.journal.is_empty() {
            return Err("cannot forward a durable input stream after reading from it".to_string());
        }
        Ok(self.handle.clone())
    }

    fn into_forwarded(self) -> Result<ForwardedDurableInput, String> {
        self.forwarded_handle()?;
        Ok(ForwardedDurableInput {
            handle: self.handle,
        })
    }
}

enum ForwardedDurableInputReference {
    Forwarded(DurableStreamHandle),
    Endpoint(DurableStreamHandle),
}

impl ForwardedDurableInputReference {
    fn handle(&self) -> &DurableStreamHandle {
        match self {
            Self::Forwarded(handle) | Self::Endpoint(handle) => handle,
        }
    }

    fn take(self, stream: &SchemaValueStream) -> Result<ForwardedDurableInput, String> {
        match self {
            Self::Forwarded(_) => stream.take_host_endpoint::<ForwardedDurableInput>(),
            Self::Endpoint(_) => stream
                .take_host_endpoint::<DurableInputEndpoint>()?
                .into_forwarded(),
        }
    }
}

fn forwarded_durable_input_reference(
    stream: &SchemaValueStream,
) -> Result<Option<ForwardedDurableInputReference>, String> {
    if stream
        .with_host_endpoint::<ForwardedDurableInput, _>(|_| ())
        .is_ok()
    {
        return stream
            .with_host_endpoint::<ForwardedDurableInput, _>(|forwarded| forwarded.handle.clone())
            .map(ForwardedDurableInputReference::Forwarded)
            .map(Some);
    }
    if stream
        .with_host_endpoint::<DurableInputEndpoint, _>(|_| ())
        .is_ok()
    {
        return stream
            .with_host_endpoint::<DurableInputEndpoint, _>(DurableInputEndpoint::forwarded_handle)?
            .map(ForwardedDurableInputReference::Endpoint)
            .map(Some);
    }
    Ok(None)
}

fn validate_forwarded_durable_input_schemas(
    value: &SchemaValue,
    graph: &SchemaGraph,
    root: &SchemaType,
    mismatch_error: &str,
) -> Result<(), String> {
    let mut seen_streams = HashSet::new();
    encode_recursive_stream_value_with_schema(value, graph, root, |stream, path| {
        if !seen_streams.insert(stream.cell_id()) {
            return Err(
                "the same affine stream appeared more than once in one value tree".to_string(),
            );
        }
        let element = stream_element_schema(graph, root, path)?;
        let element_schema_fingerprint =
            schema_fingerprint_v1(graph, element).map_err(|error| error.to_string())?;
        match forwarded_durable_input_reference(stream)? {
            Some(forwarded)
                if forwarded.handle().format_version != DURABLE_STREAM_FORMAT_VERSION
                    || forwarded.handle().element_schema_fingerprint
                        != element_schema_fingerprint =>
            {
                return Err(mismatch_error.to_string());
            }
            Some(_) => {}
            None => stream.with_host_endpoint::<LiveStreamEndpoint, _>(|_| ())?,
        }
        Ok(0)
    })?;
    Ok(())
}

enum DurableStreamReader {
    Owned {
        reader: Box<DurableCatchUpReader>,
        source: Arc<DurableStreamStore>,
        handle: Box<DurableStreamHandle>,
        next_journal_lag_sample: Instant,
    },
    Attached(Box<AttachedDurableCatchUpReader>),
}

impl DurableStreamReader {
    fn journal_lag_sample_deadline(&mut self) -> &mut Instant {
        match self {
            Self::Owned {
                next_journal_lag_sample,
                ..
            } => next_journal_lag_sample,
            Self::Attached(reader) => &mut reader.next_journal_lag_sample,
        }
    }

    async fn journal_lag_events(
        &self,
        after: Option<golem_common::model::durable_stream::StreamOffset>,
    ) -> Result<usize, StreamStoreError> {
        match self {
            Self::Owned { source, handle, .. } => source.journal_lag_events(handle, after).await,
            Self::Attached(reader) => {
                reader
                    .source
                    .journal_lag_events(&reader.handle, after)
                    .await
            }
        }
    }

    async fn next(&mut self) -> Result<Option<CommittedProducerStreamEvent>, StreamStoreError> {
        match self {
            Self::Owned { reader, .. } => reader.next().await,
            Self::Attached(reader) => reader.next().await,
        }
    }
}

async fn record_source_journal_lag(
    reader: Option<&mut DurableStreamReader>,
    after: Option<golem_common::model::durable_stream::StreamOffset>,
    force: bool,
) {
    let lag = match reader {
        Some(reader) => {
            if !force && Instant::now() < *reader.journal_lag_sample_deadline() {
                return;
            }
            let result = reader.journal_lag_events(after).await;
            // Exact telemetry can require remote metadata IO; bound attempts, including failures.
            *reader.journal_lag_sample_deadline() = Instant::now() + Duration::from_millis(100);
            match result {
                Ok(lag) => lag,
                Err(error) => {
                    tracing::debug!(
                        error = %error,
                        "Failed to sample durable consumer journal lag"
                    );
                    return;
                }
            }
        }
        None => 0,
    };
    crate::metrics::durable_stream::record_journal_lag(lag);
}

struct AttachedDurableCatchUpReader {
    source: Arc<dyn AttachedStreamSegmentSource>,
    attachment: StreamAttachmentKey,
    handle: DurableStreamHandle,
    consumer_producer: Option<Arc<DurableStreamStore>>,
    after: Option<golem_common::model::durable_stream::StreamOffset>,
    buffered: VecDeque<CommittedProducerStreamEvent>,
    terminal: bool,
    next_journal_lag_sample: Instant,
}

impl AttachedDurableCatchUpReader {
    async fn source_unavailable_overlay(
        &self,
    ) -> Result<Option<CommittedProducerStreamEvent>, StreamStoreError> {
        let Some(producer) = &self.consumer_producer else {
            return Ok(None);
        };
        let source_offset = producer
            .consumer_source_unavailable(&self.attachment)
            .await?;
        Ok(source_offset.map(|offset| CommittedProducerStreamEvent {
            stream_id: self.handle.stream_id,
            producer_sequence: 0,
            offset,
            packed_u8_batch_end: None,
            terminal_author: None,
            nested_handles: Vec::new(),
            payload: CommittedProducerStreamEventPayload::Cancel {
                role: StreamCancelRole::System,
                reason: StreamCancelReason::SourceUnavailable,
                details: None,
            },
        }))
    }

    async fn next(&mut self) -> Result<Option<CommittedProducerStreamEvent>, StreamStoreError> {
        loop {
            if let Some(event) = self.buffered.pop_front() {
                self.after = Some(event.offset);
                self.terminal = matches!(
                    event.payload,
                    CommittedProducerStreamEventPayload::End(_)
                        | CommittedProducerStreamEventPayload::Cancel { .. }
                );
                return Ok(Some(event));
            }
            if self.terminal {
                return Ok(None);
            }
            if let Some(event) = self.source_unavailable_overlay().await? {
                self.buffered.push_back(event);
                continue;
            }
            let events = match self
                .source
                .wait_for_attached_segment(
                    &self.attachment,
                    &self.handle,
                    Timestamp::now_utc().to_millis(),
                    self.after,
                )
                .await
            {
                Ok(events) => events,
                Err(error) => {
                    if let Some(event) = self.source_unavailable_overlay().await? {
                        self.buffered.push_back(event);
                        continue;
                    }
                    return Err(error);
                }
            };
            self.buffered.extend(events);
        }
    }
}

type DurableReceiveFuture = Pin<
    Box<
        dyn Future<
                Output = Result<
                    (
                        Option<DurableStreamReader>,
                        Option<CommittedProducerStreamEvent>,
                        HashMap<u64, DurableInputEndpoint>,
                        bool,
                        VecDeque<CommittedProducerStreamEvent>,
                    ),
                    String,
                >,
            > + Send
            + 'static,
    >,
>;

/// Wasmtime input producer backed by a durable session endpoint.
pub(crate) struct DurableInputProducer {
    reader: Option<DurableStreamReader>,
    journal: VecDeque<CommittedProducerStreamEvent>,
    pending: Option<DurableReceiveFuture>,
    streams: StreamSession,
    transport_stream_id: u64,
    handle: DurableStreamHandle,
    consumer_read_ordinal: u64,
    role: SessionStreamRole,
    finished: bool,
    dropping: bool,
    drop_event_sink: Option<mpsc::UnboundedSender<DropEvent>>,
    runtime_teardown: Arc<dyn Fn() -> bool + Send + Sync>,
}

/// Deferred cleanup returned when a guest drops an unread durable input.
pub struct DroppedDurableInput {
    streams: StreamSession,
    transport_stream_id: u64,
    role: StreamCancelRole,
}

impl std::fmt::Debug for DroppedDurableInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DroppedDurableInput")
            .field("transport_stream_id", &self.transport_stream_id)
            .field("role", &self.role)
            .finish_non_exhaustive()
    }
}

impl DroppedDurableInput {
    /// Cancels the durable source of a readable stream end the guest dropped without draining.
    ///
    /// The cleanup is best effort: once the session's attachment has been fenced (the invocation
    /// finished or another attempt took over, which is also what a replaying guest observes when
    /// it drops the same reader again), the cancellation is no longer ours to author and is
    /// skipped instead of failing the worker.
    pub(crate) async fn cancel(&self) -> Result<(), String> {
        match self
            .streams
            .cancel_stream(
                self.transport_stream_id,
                self.role,
                StreamCancelReason::GuestDrop,
                Some("guest dropped its durable readable stream end".to_string()),
                None,
            )
            .await
        {
            Ok(()) => Ok(()),
            Err(error) if error.starts_with("StaleEpoch:") => {
                tracing::debug!(
                    transport_stream_id = self.transport_stream_id,
                    error = %error,
                    "Skipping cancellation of dropped durable stream"
                );
                Ok(())
            }
            Err(error) => {
                tracing::warn!(
                    transport_stream_id = self.transport_stream_id,
                    role = ?self.role,
                    error = %error,
                    "Cancellation of dropped durable stream failed"
                );
                Err(error)
            }
        }
    }
}

fn durable_stream_cancel_error(
    role: StreamCancelRole,
    reason: StreamCancelReason,
    details: Option<String>,
) -> anyhow::Error {
    anyhow::Error::new(ClassifiedHostError {
        kind: HostFailureKind::Permanent,
        message: format!(
            "durable stream cancelled ({role:?}, {reason:?}): {}",
            details.unwrap_or_default()
        ),
    })
}

impl DurableInputProducer {
    /// Explicit guest resource drop also cancels endpoints that were never unwrapped.
    /// A transferred endpoint has already been taken from the affine stream cell.
    pub(crate) fn drop_unread(
        stream: SchemaValueStream,
        drop_event_sink: mpsc::UnboundedSender<DropEvent>,
        runtime_teardown: Arc<dyn Fn() -> bool + Send + Sync>,
    ) {
        if stream
            .with_host_endpoint::<DurableInputEndpoint, _>(|_| ())
            .is_ok()
            && let Ok(endpoint) = stream.take_host_endpoint::<DurableInputEndpoint>()
        {
            drop(Self::new(endpoint).with_drop_cleanup(drop_event_sink, runtime_teardown));
        }
    }

    /// Creates a producer for an already materialized session endpoint.
    pub(crate) fn new(endpoint: DurableInputEndpoint) -> Self {
        Self {
            reader: endpoint.reader,
            journal: endpoint.journal,
            pending: None,
            streams: endpoint.streams,
            transport_stream_id: endpoint.transport_stream_id,
            handle: endpoint.handle,
            consumer_read_ordinal: endpoint.consumer_read_ordinal,
            role: endpoint.role,
            finished: false,
            dropping: false,
            drop_event_sink: None,
            runtime_teardown: Arc::new(|| false),
        }
    }

    /// Installs cancellation cleanup to run when the guest drops unread input.
    pub(crate) fn with_drop_cleanup(
        mut self,
        drop_event_sink: mpsc::UnboundedSender<DropEvent>,
        runtime_teardown: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        self.drop_event_sink = Some(drop_event_sink);
        self.runtime_teardown = runtime_teardown;
        self
    }

    #[cfg(test)]
    fn begin_receive(&mut self) {
        self.begin_receive_with_registration(None);
    }

    fn begin_receive_with_registration(
        &mut self,
        source_wait: Option<SuspendableWaitRegistration>,
    ) {
        let mut reader = self.reader.take();
        let queued_event = self.journal.pop_front();
        let streams = self.streams.clone();
        let stream_id = self.handle.stream_id;
        let ordinal = self.consumer_read_ordinal;
        let role = self.role;
        self.pending = Some(Box::pin(async move {
            let mut journaled = queued_event.is_some();
            let event = match queued_event {
                Some(event) => Some(event),
                None => {
                    let result = reader
                        .as_mut()
                        .expect("durable input reader is missing")
                        .next()
                        .await;
                    drop(source_wait);
                    result.map_err(|error| error.to_string())?
                }
            };
            if event.as_ref().is_some_and(|event| {
                matches!(
                    &event.payload,
                    CommittedProducerStreamEventPayload::Cancel {
                        role: StreamCancelRole::System,
                        reason: StreamCancelReason::SourceUnavailable,
                        ..
                    }
                )
            }) {
                journaled = true;
            }
            let mut endpoints = HashMap::new();
            let mut queued_events = VecDeque::<CommittedProducerStreamEvent>::new();
            if let Some(event) = &event {
                let record = match &event.payload {
                    CommittedProducerStreamEventPayload::Value(bytes) => {
                        let value = ProtoSchemaValue::decode(bytes.as_slice())
                            .map_err(|error| format!("invalid durable stream value: {error}"))?;
                        let handle_indices = preflight_proto_recursive_stream_value(&value)?;
                        if handle_indices.len() != event.nested_handles.len() {
                            return Err(
                                "nested durable input handle count does not match the canonical value"
                                    .to_string(),
                            );
                        }
                        let mut recursive_mappings = Vec::with_capacity(event.nested_handles.len());
                        for (position, (handle_index, handle)) in handle_indices
                            .into_iter()
                            .zip(event.nested_handles.iter().cloned())
                            .enumerate()
                        {
                            if handle_index != position as u64 {
                                return Err(format!(
                                    "invalid canonical nested input handle index {handle_index}"
                                ));
                            }
                            let mapping = streams.ensure_nested_mapping(None, handle, role).await?;
                            endpoints.insert(
                                handle_index,
                                streams.endpoint(mapping.handle.clone(), 0, role).await?,
                            );
                            recursive_mappings.push(mapping);
                        }
                        StreamSessionRecord::ConsumerItemValue(StreamConsumerItemValueRecord {
                            format_version: 1,
                            session_key: streams.session_key.clone(),
                            stream_id,
                            source_offset: event.offset,
                            consumer_read_ordinal: ordinal,
                            value: bytes.clone(),
                            packed_u8: false,
                            recursive_handles: event.nested_handles.clone(),
                            recursive_mappings,
                        })
                    }
                    CommittedProducerStreamEventPayload::PackedU8(byte) => {
                        let mut bytes = vec![*byte];
                        if !journaled {
                            let batch_end = event.packed_u8_batch_end.ok_or_else(|| {
                                "packed-u8 durable input event is missing its batch boundary"
                                    .to_string()
                            })?;
                            if batch_end.producer_oplog_index()
                                != event.offset.producer_oplog_index()
                                || batch_end.sub_index() < event.offset.sub_index()
                            {
                                return Err(
                                    "packed-u8 durable input batch boundary is invalid".to_string()
                                );
                            }
                            while queued_events
                                .back()
                                .map(|event| event.offset)
                                .unwrap_or(event.offset)
                                != batch_end
                            {
                                if bytes.len() >= MAX_PACKED_U8_STREAM_ITEM_SIZE {
                                    return Err(
                                        "packed-u8 durable input batch exceeds its size limit"
                                            .to_string(),
                                    );
                                }
                                let next = reader
                                    .as_mut()
                                    .expect("durable input reader is missing")
                                    .next()
                                    .await
                                    .map_err(|error| error.to_string())?
                                    .ok_or_else(|| {
                                        "packed-u8 durable input batch ended before its boundary"
                                            .to_string()
                                    })?;
                                let expected_sub_index = event
                                    .offset
                                    .sub_index()
                                    .checked_add(bytes.len() as u32)
                                    .ok_or_else(|| {
                                        "packed-u8 consumer journal offset overflow".to_string()
                                    })?;
                                let next_byte = match &next.payload {
                                    CommittedProducerStreamEventPayload::PackedU8(byte)
                                        if next.offset.producer_oplog_index()
                                            == event.offset.producer_oplog_index()
                                            && next.offset.sub_index() == expected_sub_index
                                            && next.packed_u8_batch_end == Some(batch_end)
                                            && next.nested_handles.is_empty() =>
                                    {
                                        Some(*byte)
                                    }
                                    _ => None,
                                };
                                if let Some(byte) = next_byte {
                                    bytes.push(byte);
                                    queued_events.push_back(next);
                                } else {
                                    return Err("packed-u8 durable input batch is not contiguous"
                                        .to_string());
                                }
                            }
                        }
                        StreamSessionRecord::ConsumerItemValue(StreamConsumerItemValueRecord {
                            format_version: 1,
                            session_key: streams.session_key.clone(),
                            stream_id,
                            source_offset: event.offset,
                            consumer_read_ordinal: ordinal,
                            value: bytes,
                            packed_u8: true,
                            recursive_handles: Vec::new(),
                            recursive_mappings: Vec::new(),
                        })
                    }
                    CommittedProducerStreamEventPayload::End(result) => {
                        StreamSessionRecord::ConsumerTerminal(StreamConsumerTerminalRecord {
                            format_version: 1,
                            session_key: streams.session_key.clone(),
                            stream_id,
                            source_offset: event.offset,
                            consumer_read_ordinal: ordinal,
                            terminal: StreamConsumerTerminal::End(result.clone()),
                        })
                    }
                    CommittedProducerStreamEventPayload::Cancel {
                        role,
                        reason,
                        details,
                    } => StreamSessionRecord::ConsumerTerminal(StreamConsumerTerminalRecord {
                        format_version: 1,
                        session_key: streams.session_key.clone(),
                        stream_id,
                        source_offset: event.offset,
                        consumer_read_ordinal: ordinal,
                        terminal: StreamConsumerTerminal::Cancel {
                            role: *role,
                            reason: *reason,
                            details: details.clone(),
                        },
                    }),
                };
                if !journaled {
                    streams.append_record(None, record).await;
                    streams.commit_consumer_journal().await?;
                    let committed_through = queued_events
                        .back()
                        .map(|event| event.offset)
                        .unwrap_or(event.offset);
                    record_source_journal_lag(
                        reader.as_mut(),
                        Some(committed_through),
                        event.is_terminal(),
                    )
                    .await;
                    tracing::debug!(
                        stream_id = %stream_id,
                        source_offset = %event.offset,
                        consumer_read_ordinal = ordinal,
                        logical_item_count = queued_events.len() + 1,
                        replayed = false,
                        "Durable consumer value journal committed before guest delivery"
                    );
                } else {
                    tracing::debug!(
                        stream_id = %stream_id,
                        source_offset = %event.offset,
                        consumer_read_ordinal = ordinal,
                        replayed = true,
                        "Durable consumer value journal replayed before guest delivery"
                    );
                }
            }
            Ok((reader, event, endpoints, journaled, queued_events))
        }));
    }
}

impl Drop for DurableInputProducer {
    fn drop(&mut self) {
        if self.finished || (self.runtime_teardown)() {
            return;
        }
        let Some(drop_event_sink) = &self.drop_event_sink else {
            return;
        };
        let role = match self.role {
            SessionStreamRole::Input => StreamCancelRole::InputConsumer,
            SessionStreamRole::Output => StreamCancelRole::OutputConsumer,
        };
        let _ = drop_event_sink.send(DropEvent::CancelDroppedDurableInput {
            cancellation: Box::new(DroppedDurableInput {
                streams: self.streams.clone(),
                transport_stream_id: self.transport_stream_id,
                role,
            }),
        });
    }
}

impl<Ctx: WorkerCtx> StreamProducer<Ctx> for DurableInputProducer {
    type Item = wire::SchemaValueTree;
    type Buffer = Option<wire::SchemaValueTree>;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, Ctx>,
        mut destination: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if self.finished {
            return Poll::Ready(Ok(StreamResult::Dropped));
        }
        if finish {
            if !self.dropping {
                self.dropping = true;
                self.pending = None;
                let streams = self.streams.clone();
                let transport_stream_id = self.transport_stream_id;
                let role = match self.role {
                    SessionStreamRole::Input => StreamCancelRole::InputConsumer,
                    SessionStreamRole::Output => StreamCancelRole::OutputConsumer,
                };
                self.pending = Some(Box::pin(async move {
                    streams
                        .cancel_stream(
                            transport_stream_id,
                            role,
                            StreamCancelReason::GuestDrop,
                            Some("guest dropped its durable readable stream end".to_string()),
                            None,
                        )
                        .await?;
                    Ok((None, None, HashMap::new(), false, VecDeque::new()))
                }));
            }
            match self
                .pending
                .as_mut()
                .expect("drop cancellation is missing")
                .as_mut()
                .poll(cx)
            {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => {
                    return Poll::Ready(Err(wasmtime::Error::msg(error)));
                }
                Poll::Ready(Ok(_)) => {
                    self.finished = true;
                    self.pending = None;
                    self.reader = None;
                    return Poll::Ready(Ok(StreamResult::Cancelled));
                }
            }
        }
        if self.pending.is_none() {
            let source_wait = self.journal.is_empty().then(|| {
                store
                    .data_mut()
                    .durable_ctx_mut()
                    .state
                    .register_passive_suspendable_wait()
            });
            self.begin_receive_with_registration(source_wait);
        }
        let (reader, event, mut endpoints, _journaled, queued_events) =
            match self.pending.as_mut().unwrap().as_mut().poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Ok(result)) => result,
                Poll::Ready(Err(error)) => {
                    self.finished = true;
                    return Poll::Ready(Err(wasmtime::Error::msg(error)));
                }
            };
        self.pending = None;
        self.reader = reader;
        self.journal.extend(queued_events);
        let Some(event) = event else {
            self.finished = true;
            return Poll::Ready(Err(wasmtime::Error::msg(
                "durable input stream source closed without a terminal event",
            )));
        };
        self.consumer_read_ordinal += 1;

        let value = match event.payload {
            CommittedProducerStreamEventPayload::Value(bytes) => {
                let value = match ProtoSchemaValue::decode(bytes.as_slice()) {
                    Ok(value) => value,
                    Err(error) => {
                        self.finished = true;
                        return Poll::Ready(Err(wasmtime::Error::msg(format!(
                            "invalid durable stream value: {error}"
                        ))));
                    }
                };
                match decode_recursive_stream_value(value, |stream_id, _| {
                    endpoints
                        .remove(&stream_id)
                        .map(SchemaValueStream::from_host_endpoint)
                        .ok_or_else(|| format!("unknown nested stream reference {stream_id}"))
                }) {
                    Ok(value) => value,
                    Err(error) => {
                        self.finished = true;
                        return Poll::Ready(Err(wasmtime::Error::msg(error)));
                    }
                }
            }
            CommittedProducerStreamEventPayload::PackedU8(byte) => SchemaValue::U8(byte),
            CommittedProducerStreamEventPayload::End(StreamEndResult::Ok) => {
                self.finished = true;
                return Poll::Ready(Ok(StreamResult::Dropped));
            }
            CommittedProducerStreamEventPayload::End(StreamEndResult::ErrorContext(error)) => {
                self.finished = true;
                return Poll::Ready(Err(wasmtime::Error::msg(format!(
                    "durable stream ended with error context: {error:?}"
                ))));
            }
            CommittedProducerStreamEventPayload::Cancel {
                role: StreamCancelRole::InputProducer | StreamCancelRole::OutputProducer,
                ..
            } => {
                self.finished = true;
                return Poll::Ready(Ok(StreamResult::Dropped));
            }
            CommittedProducerStreamEventPayload::Cancel {
                role: StreamCancelRole::InputConsumer | StreamCancelRole::OutputConsumer,
                ..
            } => {
                self.finished = true;
                return Poll::Ready(Ok(StreamResult::Cancelled));
            }
            CommittedProducerStreamEventPayload::Cancel {
                role: role @ StreamCancelRole::System,
                reason,
                details,
            } => {
                self.finished = true;
                return Poll::Ready(Err(wasmtime::Error::from_anyhow(
                    durable_stream_cancel_error(role, reason, details),
                )));
            }
        };
        let encoded = {
            let mut resolver = StoreValueResolver::new(&mut store);
            match encode_value_with_streams(&value, &mut resolver) {
                Ok(encoded) => encoded,
                Err(error) => {
                    self.finished = true;
                    return Poll::Ready(Err(wasmtime::Error::msg(error.to_string())));
                }
            }
        };
        destination.set_buffer(Some(encoded));
        Poll::Ready(Ok(StreamResult::Completed))
    }

    fn try_into(mut me: Pin<Box<Self>>, ty: TypeId) -> Result<Box<dyn Any>, Pin<Box<Self>>> {
        let producer = me.as_ref().get_ref();
        if ty == TypeId::of::<ForwardedDurableInput>()
            && producer.consumer_read_ordinal == 0
            && producer.journal.is_empty()
            && producer.pending.is_none()
            && !producer.finished
        {
            let handle = producer.handle.clone();
            me.as_mut().get_mut().finished = true;
            Ok(Box::new(ForwardedDurableInput { handle }))
        } else {
            Err(me)
        }
    }
}

fn stream_element_schema<'a>(
    graph: &'a SchemaGraph,
    root: &'a SchemaType,
    path: &[StreamValuePathStep],
) -> Result<Option<&'a SchemaType>, String> {
    let mut current = root;
    for step in path {
        current = graph
            .resolve_ref(current)
            .map_err(|error| error.to_string())?;
        current = match (step, current) {
            (StreamValuePathStep::RecordField(index), SchemaType::Record { fields, .. }) => {
                &fields
                    .get(*index as usize)
                    .ok_or_else(|| "stream record path is out of range".to_string())?
                    .body
            }
            (StreamValuePathStep::VariantCasePayload(index), SchemaType::Variant { cases, .. }) => {
                cases
                    .get(*index as usize)
                    .and_then(|case| case.payload.as_ref())
                    .ok_or_else(|| "stream variant path has no payload".to_string())?
            }
            (StreamValuePathStep::TupleElement(index), SchemaType::Tuple { elements, .. }) => {
                elements
                    .get(*index as usize)
                    .ok_or_else(|| "stream tuple path is out of range".to_string())?
            }
            (StreamValuePathStep::ListElement(_), SchemaType::List { element, .. })
            | (StreamValuePathStep::FixedListElement(_), SchemaType::FixedList { element, .. }) => {
                element
            }
            (
                StreamValuePathStep::MapEntry {
                    side: golem_common::model::durable_stream::StreamMapSide::Key,
                    ..
                },
                SchemaType::Map { key, .. },
            ) => key,
            (
                StreamValuePathStep::MapEntry {
                    side: golem_common::model::durable_stream::StreamMapSide::Value,
                    ..
                },
                SchemaType::Map { value, .. },
            ) => value,
            (StreamValuePathStep::OptionSome, SchemaType::Option { inner, .. }) => inner,
            (StreamValuePathStep::ResultOk, SchemaType::Result { spec, .. }) => spec
                .ok
                .as_deref()
                .ok_or_else(|| "stream result ok path has no payload".to_string())?,
            (StreamValuePathStep::ResultErr, SchemaType::Result { spec, .. }) => spec
                .err
                .as_deref()
                .ok_or_else(|| "stream result error path has no payload".to_string())?,
            (StreamValuePathStep::UnionBranch(index), SchemaType::Union { spec, .. }) => {
                &spec
                    .branches
                    .get(*index as usize)
                    .ok_or_else(|| "stream union path is out of range".to_string())?
                    .body
            }
            _ => return Err("stream value path does not match the pinned schema".to_string()),
        };
    }
    match graph
        .resolve_ref(current)
        .map_err(|error| error.to_string())?
    {
        SchemaType::Stream { inner, .. } => Ok(inner.as_deref()),
        _ => Err("stream reference is not at a stream node in the pinned schema".to_string()),
    }
}

/// Removes runtime stream resources from a result before persisting the RPC value.
pub(crate) fn strip_streams(value: SchemaValue) -> SchemaValue {
    match value {
        SchemaValue::Stream(_) => SchemaValue::Tuple {
            elements: Vec::new(),
        },
        SchemaValue::Record { fields } => SchemaValue::Record {
            fields: fields.into_iter().map(strip_streams).collect(),
        },
        SchemaValue::Variant(mut value) => {
            value.payload = value
                .payload
                .map(|payload| Box::new(strip_streams(*payload)));
            SchemaValue::Variant(value)
        }
        SchemaValue::Tuple { elements } => SchemaValue::Tuple {
            elements: elements.into_iter().map(strip_streams).collect(),
        },
        SchemaValue::List { elements } => SchemaValue::List {
            elements: elements.into_iter().map(strip_streams).collect(),
        },
        SchemaValue::FixedList { elements } => SchemaValue::FixedList {
            elements: elements.into_iter().map(strip_streams).collect(),
        },
        SchemaValue::Map { entries } => SchemaValue::Map {
            entries: entries
                .into_iter()
                .map(|(key, value)| (strip_streams(key), strip_streams(value)))
                .collect(),
        },
        SchemaValue::Option { inner } => SchemaValue::Option {
            inner: inner.map(|inner| Box::new(strip_streams(*inner))),
        },
        SchemaValue::Result(mut result) => {
            match &mut result {
                golem_common::schema::schema_value::ResultValuePayload::Ok { value }
                | golem_common::schema::schema_value::ResultValuePayload::Err { value } => {
                    *value = value.take().map(|value| Box::new(strip_streams(*value)));
                }
            }
            SchemaValue::Result(result)
        }
        SchemaValue::Union(mut value) => {
            value.body = Box::new(strip_streams(*value.body));
            SchemaValue::Union(value)
        }
        other => other,
    }
}

fn discards_input_after_terminal(error: &StreamStoreError, session_key: &StreamSessionKey) -> bool {
    matches!(
        error,
        StreamStoreError::SessionFinished(finished) if finished == session_key
    ) || matches!(
        error,
        StreamStoreError::ClosedByOtherProducer
            | StreamStoreError::FencedByTerminal(CommittedProducerStreamEventPayload::Cancel {
                role: StreamCancelRole::InputConsumer,
                ..
            })
    )
}

fn collect_stream_paths(
    value: &ProtoSchemaValue,
    graph: &SchemaGraph,
    root: &SchemaType,
) -> Result<Vec<(u64, Vec<StreamValuePathStep>)>, String> {
    let mut result = Vec::new();
    decode_recursive_stream_value_with_schema(value.clone(), graph, root, |stream_id, path| {
        result.push((stream_id, path.to_vec()));
        Ok(SchemaValueStream::from_host_endpoint(()))
    })?;
    Ok(result)
}

#[cfg(test)]
mod tests;
