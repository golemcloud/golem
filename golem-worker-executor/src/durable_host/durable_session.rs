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

use crate::durable_host::durable_stream::{
    AttachedStreamSegmentSource, CommittedProducerStreamEventPayloadV1,
    CommittedProducerStreamEventV1, ConsumerAttachmentStatus, DurableCatchUpReader,
    DurableStreamProducer, DurableStreamProducerError, NestedStreamWriteV1,
    ProducerRegistrationRequestV1, RoutedAttachedStreamSegmentSource,
    RoutedStreamAttachmentControl, StreamAttachmentConsumerProbe, StreamAttachmentControl,
    StreamAttachmentStateV1, StreamSegmentSource,
};
use crate::durable_host::schema_value_stream::StoreValueResolver;
use crate::durable_host::stream_bus::{LiveStreamEventPayload, LiveStreamReceiveError};
use crate::durable_host::stream_session::{
    decode_recursive_stream_value, decode_recursive_stream_value_with_schema,
    encode_recursive_stream_value_with_schema, preflight_proto_recursive_stream_value,
    preflight_recursive_stream_value, remap_recursive_stream_references,
};
use crate::durable_host::stream_transport::{LiveStreamEndpoint, SourceLifecycle};
use crate::services::oplog::{Oplog, OplogOps};
use crate::services::rpc::Rpc;
use crate::workerctx::WorkerCtx;
use futures::future::try_join_all;
use golem_api_grpc::proto::golem::schema::SchemaValue as ProtoSchemaValue;
use golem_api_grpc::proto::golem::worker::{
    DurableStreamHandle, DurableStreamMapping, InputStreamHighWater, InvocationResponse,
    OutputStreamEnd, OutputStreamError, OutputStreamItem, StreamCancel, StreamInvocationIdentity,
    StreamMappingRole, invocation_response,
};
use golem_common::base_model::durable_stream::{
    AttachmentId, AttemptId, DURABLE_STREAM_FORMAT_VERSION, DurableStreamHandleV1,
    InputStreamHighWaterV1, MAX_NEW_STREAM_HANDLES_PER_VALUE, SessionStreamRoleV1,
    StreamAttachmentKeyV1, StreamCallerAttemptRecordV1, StreamCancelReasonV1, StreamCancelRoleV1,
    StreamConsumerCancelIntentRecordV1, StreamConsumerItemValueRecordV1,
    StreamConsumerTerminalRecordV1, StreamConsumerTerminalV1, StreamEndResultV1,
    StreamInvocationIdV1, StreamItemsPayloadV1, StreamRegistrationCoordinateV1,
    StreamResumeOperationV1, StreamRootKindV1, StreamSessionDetachedRecordV1,
    StreamSessionInvocationResultRecordV1, StreamSessionKeyV1, StreamSessionMappingRecordV1,
    StreamSessionMappingUpdateRecordV1, StreamSessionMappingV1, StreamSessionRecordV1,
    StreamSessionResumeAttemptRecordV1, StreamSourceKindV1, StreamTopologyActivatedRecordV1,
    StreamTopologyPreparedRecordV1, StreamValuePathStepV1,
};
use golem_common::base_model::oplog::OplogEntry;
use golem_common::model::Timestamp;
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
use tokio::sync::{Mutex, mpsc};
use wasmtime::StoreContextMut;
use wasmtime::component::{Destination, StreamProducer, StreamResult};

#[async_trait::async_trait]
pub(crate) trait DurableStreamConsumerJournal: Send + Sync {
    async fn commit(&self) -> Result<(), String>;

    async fn source_unavailable(
        &self,
        _key: &StreamAttachmentKeyV1,
    ) -> Result<Option<golem_common::model::durable_stream::StreamOffsetV1>, String> {
        Ok(None)
    }
}

#[derive(Clone)]
pub(crate) struct DurableSessionStreams {
    pub(crate) producer: Arc<DurableStreamProducer>,
    pub(crate) oplog: Arc<dyn Oplog>,
    pub(crate) session_key: StreamSessionKeyV1,
    consumer_invocation: StreamInvocationIdV1,
    mappings: Arc<RwLock<HashMap<u64, (DurableStreamHandleV1, SessionStreamRoleV1)>>>,
    input_schema: Option<Arc<DurableInputSchema>>,
    rpc: Option<Arc<dyn Rpc>>,
    consumer_journal: Option<Arc<dyn DurableStreamConsumerJournal>>,
    auth_ctx: Option<AuthCtx>,
    require_attachment_before_production: bool,
    next_transport_stream_id: Arc<AtomicU64>,
    session_lock: Arc<Mutex<()>>,
    attachment_epoch: u64,
    attachment_attempt_id: Option<AttemptId>,
}

struct DurableInputSchema {
    graph: Arc<SchemaGraph>,
    component_revision: golem_common::model::component::ComponentRevision,
    element_types: RwLock<HashMap<u64, SchemaType>>,
}

struct OutputDrainRegistration {
    producer: Arc<DurableStreamProducer>,
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
    handle: DurableStreamHandleV1,
    endpoint: LiveStreamEndpoint,
    element_type: SchemaType,
    role: SessionStreamRoleV1,
}

pub(crate) fn durable_stream_mapping_to_proto(
    mapping: &StreamSessionMappingRecordV1,
    high_water: Option<&InputStreamHighWaterV1>,
) -> DurableStreamMapping {
    DurableStreamMapping {
        transport_stream_id: mapping.transport_stream_id,
        handle: Some(DurableStreamHandle {
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
        high_water: high_water.map(|high_water| InputStreamHighWater {
            highest_contiguous_sequence: high_water.highest_contiguous_sequence,
            resulting_offset: high_water.resulting_offset.as_bytes().to_vec(),
            terminal: high_water.terminal,
        }),
        role: match mapping.role {
            SessionStreamRoleV1::Input => StreamMappingRole::Input as i32,
            SessionStreamRoleV1::Output => StreamMappingRole::Output as i32,
        },
    }
}

pub(crate) fn durable_stream_mapping_from_proto(
    mapping: DurableStreamMapping,
) -> Result<StreamSessionMappingRecordV1, String> {
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
            StreamMappingRole::Input => SessionStreamRoleV1::Input,
            StreamMappingRole::Output => SessionStreamRoleV1::Output,
            StreamMappingRole::Unspecified => {
                return Err("durable stream mapping has no role".to_string());
            }
        };
    Ok(StreamSessionMappingRecordV1 {
        transport_stream_id: mapping.transport_stream_id,
        handle: DurableStreamHandleV1 {
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
            source_invocation: StreamInvocationIdV1 {
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

impl DurableSessionStreams {
    pub(crate) fn new(
        producer: Arc<DurableStreamProducer>,
        oplog: Arc<dyn Oplog>,
        session_key: StreamSessionKeyV1,
        mappings: impl IntoIterator<Item = (u64, DurableStreamHandleV1, SessionStreamRoleV1)>,
    ) -> Self {
        let session_lock = producer.session_lock(&session_key);
        let mappings = mappings
            .into_iter()
            .map(|(transport_stream_id, handle, role)| (transport_stream_id, (handle, role)))
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
            require_attachment_before_production: false,
            next_transport_stream_id: Arc::new(AtomicU64::new(next_transport_stream_id)),
            session_lock,
            attachment_epoch: 1,
            attachment_attempt_id: None,
        }
    }

    pub(crate) fn with_attachment(mut self, epoch: u64, attempt_id: AttemptId) -> Self {
        self.attachment_epoch = epoch;
        self.attachment_attempt_id = Some(attempt_id);
        self
    }

    pub(crate) fn with_consumer_invocation(
        mut self,
        consumer_invocation: StreamInvocationIdV1,
    ) -> Self {
        self.consumer_invocation = consumer_invocation;
        self
    }

    pub(crate) fn with_rpc(mut self, rpc: Arc<dyn Rpc>) -> Self {
        self.rpc = Some(rpc);
        self
    }

    pub(crate) fn with_consumer_journal(
        mut self,
        consumer_journal: Arc<dyn DurableStreamConsumerJournal>,
    ) -> Self {
        self.consumer_journal = Some(consumer_journal);
        self
    }

    pub(crate) async fn commit_consumer_journal(&self) -> Result<(), String> {
        self.consumer_journal
            .as_ref()
            .ok_or_else(|| "durable stream consumer journal commit is unavailable".to_string())?
            .commit()
            .await
    }

    pub(crate) fn with_auth_ctx(mut self, auth_ctx: AuthCtx) -> Self {
        self.auth_ctx = Some(auth_ctx);
        self
    }

    pub(crate) fn require_attachment_before_production(mut self) -> Self {
        self.require_attachment_before_production = true;
        self
    }

    fn allocate_transport_stream_id(&self) -> Result<u64, String> {
        self.next_transport_stream_id
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(1)
            })
            .map_err(|_| "durable transport stream id overflow".to_string())
    }

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

    pub(crate) fn handle(&self, transport_stream_id: u64) -> Option<DurableStreamHandleV1> {
        self.mappings
            .read()
            .expect("durable stream mapping lock poisoned")
            .get(&transport_stream_id)
            .map(|(handle, _)| handle.clone())
    }

    fn mapping(&self, transport_stream_id: u64) -> Option<StreamSessionMappingRecordV1> {
        self.mappings
            .read()
            .expect("durable stream mapping lock poisoned")
            .get(&transport_stream_id)
            .map(|(handle, role)| StreamSessionMappingRecordV1 {
                transport_stream_id,
                handle: handle.clone(),
                role: *role,
            })
    }

    fn mapping_for_handle(
        &self,
        handle: &DurableStreamHandleV1,
        role: SessionStreamRoleV1,
    ) -> Option<StreamSessionMappingRecordV1> {
        self.mappings
            .read()
            .expect("durable stream mapping lock poisoned")
            .iter()
            .find_map(|(transport_stream_id, (candidate, candidate_role))| {
                (candidate == handle && *candidate_role == role).then(|| {
                    StreamSessionMappingRecordV1 {
                        transport_stream_id: *transport_stream_id,
                        handle: candidate.clone(),
                        role,
                    }
                })
            })
    }

    pub(crate) async fn validate_frame(
        &self,
        transport_stream_id: u64,
        durable_stream_id: Option<golem_api_grpc::proto::golem::common::Uuid>,
        epoch: u64,
        expected_role: SessionStreamRoleV1,
    ) -> Result<DurableStreamHandleV1, String> {
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
        let (handle, role) = mappings
            .get(&transport_stream_id)
            .ok_or_else(|| format!("unknown durable transport stream ID {transport_stream_id}"))?;
        if handle.stream_id.0 != durable_stream_id || *role != expected_role {
            return Err(
                "transport stream mapping does not match the durable stream ID and role"
                    .to_string(),
            );
        }
        Ok(handle.clone())
    }

    pub(crate) fn attachment_epoch(&self) -> u64 {
        self.attachment_epoch
    }

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

    async fn authoritative_attachment_state(&self) -> Result<(u64, AttemptId, bool), String> {
        let current = self.oplog.current_oplog_index().await;
        if !current.is_defined() {
            return Err("durable session has no attachment authority".to_string());
        }
        let mut state = None;
        for (_, entry) in self
            .oplog
            .read_many(
                golem_common::model::oplog::OplogIndex::INITIAL,
                current.as_u64(),
            )
            .await
        {
            let OplogEntry::StreamSession { record, .. } = entry else {
                continue;
            };
            match self.download_record(record).await? {
                StreamSessionRecordV1::Attached(record)
                    if record.session_key == self.session_key =>
                {
                    if state.is_some() {
                        return Err("durable session contains a repeated initial attachment".into());
                    }
                    state = Some((record.epoch, record.attempt_id, true));
                }
                StreamSessionRecordV1::ResumeAttempt(record)
                    if record.attempt.session_key == self.session_key =>
                {
                    let Some((epoch, _, _)) = state else {
                        return Err("durable resume precedes initial attachment".into());
                    };
                    if record.attempt.expected_epoch != epoch
                        || record.accepted_epoch
                            != epoch.checked_add(1).ok_or_else(|| {
                                "durable attachment epoch cannot advance past u64::MAX".to_string()
                            })?
                    {
                        return Err("durable resume contains an invalid epoch transition".into());
                    }
                    state = Some((record.accepted_epoch, record.attempt.attempt_id, true));
                }
                StreamSessionRecordV1::Detached(record)
                    if record.session_key == self.session_key =>
                {
                    let Some((epoch, attempt_id, attached)) = state else {
                        return Err("durable detach precedes initial attachment".into());
                    };
                    if record.epoch != epoch || record.owner_attempt_id != attempt_id {
                        return Err("durable detach does not match the current attachment".into());
                    }
                    if attached {
                        state = Some((epoch, attempt_id, false));
                    }
                }
                _ => {}
            }
        }
        state.ok_or_else(|| "durable session has no attachment authority".to_string())
    }

    pub(crate) async fn detach_current(&self) -> Result<bool, String> {
        let _session_guard = self.session_lock.lock().await;
        let (epoch, attempt_id, attached) = self.authoritative_attachment_state().await?;
        if !attached
            || epoch != self.attachment_epoch
            || self.attachment_attempt_id != Some(attempt_id)
        {
            return Ok(false);
        }
        self.append_record(StreamSessionRecordV1::Detached(
            StreamSessionDetachedRecordV1 {
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
            },
        ))
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
    pub(crate) async fn commit_resume_attempt(
        &self,
        record: StreamSessionResumeAttemptRecordV1,
    ) -> Result<(), String> {
        let _session_guard = self.session_lock.lock().await;
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
            (StreamResumeOperationV1::Resume, false)
            | (StreamResumeOperationV1::Takeover, true) => {}
            _ => {
                return Err(
                    "InvalidAttachmentState: resume requires Detached and takeover requires Attached"
                        .to_string(),
                );
            }
        }
        let result = self
            .producer
            .append_session_record(StreamSessionRecordV1::ResumeAttempt(record))
            .await
            .map_err(|error| error.to_string());
        if result.is_ok() {
            tracing::debug!("Durable Stream Session resume attempt committed");
        }
        result
    }

    pub(crate) fn insert_mapping(
        &self,
        transport_stream_id: u64,
        handle: DurableStreamHandleV1,
        role: SessionStreamRoleV1,
    ) -> Result<(), String> {
        let mut mappings = self
            .mappings
            .write()
            .expect("durable stream mapping lock poisoned");
        if let Some((existing_transport_stream_id, _)) = mappings
            .iter()
            .find(|(_, existing)| existing == &&(handle.clone(), role))
            && *existing_transport_stream_id != transport_stream_id
        {
            return Err(format!(
                "durable stream is already mapped to transport stream id {existing_transport_stream_id}"
            ));
        }
        match mappings.get(&transport_stream_id) {
            Some(existing) if existing == &(handle.clone(), role) => Ok(()),
            Some(_) => Err(format!(
                "transport stream id {transport_stream_id} is already mapped to another durable stream"
            )),
            None => {
                self.next_transport_stream_id
                    .fetch_max(transport_stream_id.saturating_add(1), Ordering::AcqRel);
                mappings.insert(transport_stream_id, (handle, role));
                Ok(())
            }
        }
    }

    pub(crate) async fn append_record(&self, record: StreamSessionRecordV1) {
        self.producer
            .append_session_record(record)
            .await
            .expect("internally generated durable session record is valid");
    }

    async fn try_append_record(&self, record: StreamSessionRecordV1) -> Result<(), String> {
        self.producer
            .append_session_record(record)
            .await
            .map_err(|error| error.to_string())
    }

    pub(crate) async fn caller_attempt_id(&self) -> Result<AttemptId, String> {
        let _guard = self.session_lock.lock().await;
        let current = self.oplog.current_oplog_index().await;
        let mut persisted = None;
        if current.is_defined() {
            for (_, entry) in self
                .oplog
                .read_many(
                    golem_common::model::oplog::OplogIndex::INITIAL,
                    current.as_u64(),
                )
                .await
            {
                let OplogEntry::StreamSession { record, .. } = entry else {
                    continue;
                };
                let StreamSessionRecordV1::CallerAttempt(record) =
                    self.download_record(record).await?
                else {
                    continue;
                };
                if record.session_key != self.session_key {
                    continue;
                }
                match persisted {
                    Some(existing) if existing != record.attempt_id => {
                        return Err(
                            "conflicting caller attempt IDs are persisted for the Stream Session"
                                .to_string(),
                        );
                    }
                    Some(_) => {}
                    None => persisted = Some(record.attempt_id),
                }
            }
        }
        if let Some(attempt_id) = persisted {
            return Ok(attempt_id);
        }
        let attempt_id = AttemptId::fresh();
        self.append_record(StreamSessionRecordV1::CallerAttempt(
            StreamCallerAttemptRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: self.session_key.clone(),
                attempt_id,
            },
        ))
        .await;
        self.commit_consumer_journal().await?;
        Ok(attempt_id)
    }

    async fn download_record(
        &self,
        record: OplogPayload<StreamSessionRecordV1>,
    ) -> Result<StreamSessionRecordV1, String> {
        let record = self.oplog.download_payload(record).await?;
        if record.has_supported_format() {
            Ok(record)
        } else {
            Err("unsupported or malformed durable Stream Session record version".to_string())
        }
    }

    async fn append_mapping_once(
        &self,
        mapping: StreamSessionMappingRecordV1,
    ) -> Result<(), String> {
        let current = self.oplog.current_oplog_index().await;
        if current.is_defined() {
            for (_, entry) in self
                .oplog
                .read_many(
                    golem_common::model::oplog::OplogIndex::INITIAL,
                    current.as_u64(),
                )
                .await
            {
                let OplogEntry::StreamSession { record, .. } = entry else {
                    continue;
                };
                if let StreamSessionRecordV1::Mapping(existing) =
                    self.download_record(record).await?
                    && existing.session_key == self.session_key
                    && existing.mapping == mapping
                {
                    return Ok(());
                }
            }
        }
        self.producer
            .ensure_session_accepts_new_events(&self.session_key)
            .await
            .map_err(|error| error.to_string())?;
        self.append_record(StreamSessionRecordV1::Mapping(
            StreamSessionMappingUpdateRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: self.session_key.clone(),
                mapping,
            },
        ))
        .await;
        Ok(())
    }

    pub(crate) async fn activate_forwarded_mapping(
        &self,
        attachment: StreamAttachmentKeyV1,
        mapping: StreamSessionMappingRecordV1,
        producer_control: &(dyn StreamAttachmentControl + Send + Sync),
        now_millis: u64,
    ) -> Result<(), String> {
        let _session_guard = self.session_lock.lock().await;
        self.activate_forwarded_mapping_under_lock(
            attachment,
            mapping,
            producer_control,
            now_millis,
        )
        .await
    }

    async fn activate_forwarded_mapping_under_lock(
        &self,
        attachment: StreamAttachmentKeyV1,
        mapping: StreamSessionMappingRecordV1,
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
            self.try_append_record(StreamSessionRecordV1::TopologyPrepared(
                StreamTopologyPreparedRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: self.session_key.clone(),
                    attachment: attachment.clone(),
                    mapping: mapping.clone(),
                },
            ))
            .await?;
            self.commit_consumer_journal().await?;
        }
        producer_control
            .prepare_attachment(attachment.clone(), now_millis)
            .await
            .map_err(|error| error.to_string())?;
        if topology != ConsumerAttachmentStatus::Active {
            self.require_local_session_attachment(&attachment).await?;
            self.try_append_record(StreamSessionRecordV1::TopologyActivated(
                StreamTopologyActivatedRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: self.session_key.clone(),
                    attachment: attachment.clone(),
                    mapping: mapping.clone(),
                },
            ))
            .await?;
            self.commit_consumer_journal().await?;
        }
        producer_control
            .activate_attachment(attachment, now_millis)
            .await
            .map_err(|error| error.to_string())?;
        self.append_mapping_once(mapping.clone()).await?;
        self.commit_consumer_journal().await?;
        self.insert_mapping(mapping.transport_stream_id, mapping.handle, mapping.role)
    }

    async fn require_local_session_attachment(
        &self,
        attachment: &StreamAttachmentKeyV1,
    ) -> Result<(), String> {
        if self.session_key.callee_environment_id != self.producer.environment_id()
            || self.session_key.callee != *self.producer.agent_id()
            || self.session_key.callee_fingerprint != self.producer.fingerprint()
        {
            return Ok(());
        }
        let current = self.oplog.current_oplog_index().await;
        if !current.is_defined() {
            return Err("durable topology has no local session authority".to_string());
        }
        let mut prepared_attempt = None;
        let mut attachment_authority = None;
        for (_, entry) in self
            .oplog
            .read_many(
                golem_common::model::oplog::OplogIndex::INITIAL,
                current.as_u64(),
            )
            .await
        {
            let OplogEntry::StreamSession { record, .. } = entry else {
                continue;
            };
            match self.download_record(record).await? {
                StreamSessionRecordV1::Prepared(record)
                    if record.attempt.session_key == self.session_key =>
                {
                    if prepared_attempt
                        .replace(record.attempt.attempt_id)
                        .is_some()
                    {
                        return Err(
                            "durable Stream Session contains multiple Prepared records".to_string()
                        );
                    }
                }
                StreamSessionRecordV1::Attached(record)
                    if record.session_key == self.session_key =>
                {
                    if attachment_authority.is_some() {
                        return Err(
                            "durable Stream Session contains a repeated initial attachment"
                                .to_string(),
                        );
                    }
                    attachment_authority = Some((record.epoch, record.attempt_id, true));
                }
                StreamSessionRecordV1::ResumeAttempt(record)
                    if record.attempt.session_key == self.session_key =>
                {
                    let Some((epoch, _, _)) = attachment_authority else {
                        return Err("durable resume precedes initial attachment".to_string());
                    };
                    if record.attempt.expected_epoch != epoch
                        || record.accepted_epoch
                            != epoch.checked_add(1).ok_or_else(|| {
                                "durable attachment epoch cannot advance past u64::MAX".to_string()
                            })?
                    {
                        return Err("durable resume contains an invalid epoch transition".into());
                    }
                    attachment_authority =
                        Some((record.accepted_epoch, record.attempt.attempt_id, true));
                }
                StreamSessionRecordV1::Detached(record)
                    if record.session_key == self.session_key =>
                {
                    let Some((epoch, owner_attempt, attached)) = attachment_authority else {
                        return Err("durable detach precedes initial attachment".to_string());
                    };
                    if record.epoch != epoch || record.owner_attempt_id != owner_attempt {
                        return Err(
                            "durable detach does not match the current attachment".to_string()
                        );
                    }
                    if attached {
                        attachment_authority = Some((epoch, owner_attempt, false));
                    }
                }
                _ => {}
            }
        }
        let prepared_attempt = prepared_attempt
            .ok_or_else(|| "durable topology has no Prepared session authority".to_string())?;
        let (attached_epoch, attached_attempt_id, attached) =
            attachment_authority.ok_or_else(|| {
                "durable topology cannot activate before session attachment".to_string()
            })?;
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
                            self.initial_attached_pending_index().await?.ok_or_else(|| {
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

    async fn initial_attached_pending_index(
        &self,
    ) -> Result<Option<golem_common::model::oplog::OplogIndex>, String> {
        let current = self.oplog.current_oplog_index().await;
        if !current.is_defined() {
            return Ok(None);
        }
        for (_, entry) in self
            .oplog
            .read_many(
                golem_common::model::oplog::OplogIndex::INITIAL,
                current.as_u64(),
            )
            .await
        {
            let OplogEntry::StreamSession { record, .. } = entry else {
                continue;
            };
            if let StreamSessionRecordV1::Attached(record) = self.download_record(record).await?
                && record.session_key == self.session_key
            {
                return Ok(Some(record.pending_invocation_oplog_index));
            }
        }
        Ok(None)
    }

    fn validate_forwarded_mapping(
        &self,
        attachment: &StreamAttachmentKeyV1,
        mapping: &StreamSessionMappingRecordV1,
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
        attachment: &StreamAttachmentKeyV1,
        expected_mapping: Option<&StreamSessionMappingRecordV1>,
    ) -> Result<ConsumerAttachmentStatus, String> {
        if attachment.session_key != self.session_key
            || attachment.consumer_invocation != self.consumer_invocation
            || attachment.consumer_environment_id != self.producer.environment_id()
            || attachment.consumer != *self.producer.agent_id()
            || attachment.expected_consumer_fingerprint != self.producer.fingerprint()
        {
            return Ok(ConsumerAttachmentStatus::IncarnationMismatch);
        }
        let current = self.oplog.current_oplog_index().await;
        if !current.is_defined() {
            return Ok(ConsumerAttachmentStatus::Missing);
        }
        let mut state = ConsumerAttachmentStatus::Missing;
        let local_session_authority = self.session_key.callee_environment_id
            == self.producer.environment_id()
            && self.session_key.callee == *self.producer.agent_id()
            && self.session_key.callee_fingerprint == self.producer.fingerprint();
        let mut attached_epoch = (!local_session_authority).then_some(attachment.epoch);
        for (_, entry) in self
            .oplog
            .read_many(
                golem_common::model::oplog::OplogIndex::INITIAL,
                current.as_u64(),
            )
            .await
        {
            let OplogEntry::StreamSession { record, .. } = entry else {
                continue;
            };
            let durable_topology = match self.download_record(record).await? {
                StreamSessionRecordV1::Attached(record)
                    if record.session_key == self.session_key =>
                {
                    attached_epoch = Some(record.epoch);
                    None
                }
                StreamSessionRecordV1::ResumeAttempt(record)
                    if record.attempt.session_key == self.session_key =>
                {
                    attached_epoch = Some(record.accepted_epoch);
                    None
                }
                StreamSessionRecordV1::TopologyPrepared(record)
                    if record.session_key == self.session_key =>
                {
                    if same_attachment_slot(&record.attachment, attachment) {
                        if record.attachment.epoch < attachment.epoch {
                            continue;
                        }
                        if record.attachment != *attachment {
                            return Ok(attachment_mismatch_status(&record.attachment, attachment));
                        }
                        if expected_mapping.is_none_or(|mapping| mapping == &record.mapping)
                            && state == ConsumerAttachmentStatus::Missing
                        {
                            state = ConsumerAttachmentStatus::Prepared;
                        }
                    }
                    None
                }
                StreamSessionRecordV1::TopologyActivated(record)
                    if record.session_key == self.session_key =>
                {
                    Some((record.attachment, record.mapping))
                }
                _ => None,
            };
            if let Some((durable_attachment, durable_mapping)) = durable_topology
                && same_attachment_slot(&durable_attachment, attachment)
            {
                if durable_attachment.epoch < attachment.epoch {
                    continue;
                }
                if durable_attachment != *attachment {
                    return Ok(attachment_mismatch_status(&durable_attachment, attachment));
                }
                if expected_mapping.is_some_and(|mapping| mapping != &durable_mapping) {
                    continue;
                }
                if expected_mapping.is_none() && state == ConsumerAttachmentStatus::Active {
                    continue;
                }
                if state != ConsumerAttachmentStatus::Prepared {
                    return Err(
                        "durable topology activation has no matching preparation".to_string()
                    );
                }
                state = ConsumerAttachmentStatus::Active;
            }
        }
        match attached_epoch {
            Some(epoch) if epoch == attachment.epoch => Ok(state),
            Some(_) => Ok(ConsumerAttachmentStatus::EpochMismatch),
            None if state == ConsumerAttachmentStatus::Active => {
                Err("durable topology activation precedes session attachment".to_string())
            }
            None => Ok(state),
        }
    }

    pub(crate) async fn recover_nested_input_mappings(&self) -> Result<(), String> {
        self.recover_session_mappings().await
    }

    pub(crate) async fn recover_session_mappings(&self) -> Result<(), String> {
        let current = self.oplog.current_oplog_index().await;
        if !current.is_defined() {
            return Ok(());
        }
        for (_, entry) in self
            .oplog
            .read_many(
                golem_common::model::oplog::OplogIndex::INITIAL,
                current.as_u64(),
            )
            .await
        {
            let OplogEntry::StreamSession { record, .. } = entry else {
                continue;
            };
            match self.download_record(record).await? {
                StreamSessionRecordV1::Mapping(record)
                    if record.session_key == self.session_key =>
                {
                    self.validate_recovered_mapping(&record.mapping).await?;
                    self.insert_mapping(
                        record.mapping.transport_stream_id,
                        record.mapping.handle,
                        record.mapping.role,
                    )?;
                }
                StreamSessionRecordV1::InvocationResult(record)
                    if record.session_key == self.session_key =>
                {
                    for mapping in record.stream_mappings {
                        self.validate_recovered_mapping(&mapping).await?;
                        self.insert_mapping(
                            mapping.transport_stream_id,
                            mapping.handle,
                            mapping.role,
                        )?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    async fn validate_recovered_mapping(
        &self,
        mapping: &StreamSessionMappingRecordV1,
    ) -> Result<(), String> {
        if self.producer.owns_handle_identity(&mapping.handle) {
            return self
                .producer
                .validate_handle(&mapping.handle)
                .await
                .map_err(|error| error.to_string());
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
        handle: &DurableStreamHandleV1,
        epoch: u64,
    ) -> Result<StreamAttachmentKeyV1, String> {
        Ok(StreamAttachmentKeyV1 {
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

    pub(crate) async fn validate_resume_cursors(
        &self,
        cursors: &[golem_common::model::durable_stream::StreamResumeCursorV1],
    ) -> Result<(), String> {
        for cursor in cursors {
            let mapping = self
                .mappings
                .read()
                .expect("durable stream mapping lock poisoned")
                .iter()
                .find_map(|(transport_stream_id, (handle, role))| {
                    (handle.stream_id == cursor.stream_id).then_some(StreamSessionMappingRecordV1 {
                        transport_stream_id: *transport_stream_id,
                        handle: handle.clone(),
                        role: *role,
                    })
                })
                .ok_or_else(|| {
                    format!(
                        "resume cursor names unmapped durable stream {}",
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
                let source = RoutedAttachedStreamSegmentSource::new(rpc, mapping.clone(), auth_ctx);
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

    pub(crate) async fn activate_foreign_mapping(
        &self,
        mapping: StreamSessionMappingRecordV1,
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
            &control,
            Timestamp::now_utc().to_millis(),
        )
        .await
    }

    pub(crate) async fn prepare_foreign_mapping(
        &self,
        mapping: StreamSessionMappingRecordV1,
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
            self.try_append_record(StreamSessionRecordV1::TopologyPrepared(
                StreamTopologyPreparedRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: self.session_key.clone(),
                    attachment: attachment.clone(),
                    mapping,
                },
            ))
            .await?;
            self.commit_consumer_journal().await?;
        }
        control
            .prepare_attachment(attachment, Timestamp::now_utc().to_millis())
            .await
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    async fn persisted_session_mapping(
        &self,
        expected: &StreamSessionMappingRecordV1,
    ) -> Result<bool, String> {
        let current = self.oplog.current_oplog_index().await;
        if !current.is_defined() {
            return Ok(false);
        }
        for (_, entry) in self
            .oplog
            .read_many(
                golem_common::model::oplog::OplogIndex::INITIAL,
                current.as_u64(),
            )
            .await
        {
            let OplogEntry::StreamSession { record, .. } = entry else {
                continue;
            };
            let found = match self.download_record(record).await? {
                StreamSessionRecordV1::Prepared(record) => {
                    record.attempt.session_key == self.session_key
                        && record.stream_mappings.contains(expected)
                }
                StreamSessionRecordV1::Mapping(record) => {
                    record.session_key == self.session_key && record.mapping == *expected
                }
                StreamSessionRecordV1::TopologyPrepared(record) => {
                    record.session_key == self.session_key && record.mapping == *expected
                }
                StreamSessionRecordV1::TopologyActivated(record) => {
                    record.session_key == self.session_key && record.mapping == *expected
                }
                StreamSessionRecordV1::ConsumerItemValue(record) => {
                    record.session_key == self.session_key
                        && record.recursive_mappings.contains(expected)
                }
                StreamSessionRecordV1::InvocationResult(record) => {
                    record.session_key == self.session_key
                        && record.stream_mappings.contains(expected)
                }
                _ => false,
            };
            if found {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn ensure_nested_mapping(
        &self,
        handle: DurableStreamHandleV1,
        role: SessionStreamRoleV1,
    ) -> Result<StreamSessionMappingRecordV1, String> {
        if let Some(mapping) = self.mapping_for_handle(&handle, role) {
            return Ok(mapping);
        }
        let mapping = StreamSessionMappingRecordV1 {
            transport_stream_id: self.allocate_transport_stream_id()?,
            handle,
            role,
        };
        if self.producer.owns_handle_identity(&mapping.handle) {
            self.append_mapping_once(mapping.clone()).await?;
            self.insert_mapping(
                mapping.transport_stream_id,
                mapping.handle.clone(),
                mapping.role,
            )?;
        } else {
            self.activate_foreign_mapping(mapping.clone(), 1).await?;
        }
        Ok(mapping)
    }

    async fn ensure_nested_mapping_under_lock(
        &self,
        handle: DurableStreamHandleV1,
        role: SessionStreamRoleV1,
    ) -> Result<StreamSessionMappingRecordV1, String> {
        if let Some(mapping) = self.mapping_for_handle(&handle, role) {
            return Ok(mapping);
        }
        let mapping = StreamSessionMappingRecordV1 {
            transport_stream_id: self.allocate_transport_stream_id()?,
            handle,
            role,
        };
        if self.producer.owns_handle_identity(&mapping.handle) {
            self.append_mapping_once(mapping.clone()).await?;
            self.insert_mapping(
                mapping.transport_stream_id,
                mapping.handle.clone(),
                mapping.role,
            )?;
        } else {
            let rpc = self.rpc.clone().ok_or_else(|| {
                "foreign durable stream control routing is unavailable".to_string()
            })?;
            let auth_ctx = self.auth_ctx.clone().ok_or_else(|| {
                "foreign durable stream consumer authorization is unavailable".to_string()
            })?;
            let attachment = self.attachment_key(&mapping.handle, 1)?;
            let control = RoutedStreamAttachmentControl::new(rpc, mapping.clone(), auth_ctx);
            self.activate_forwarded_mapping_under_lock(
                attachment,
                mapping.clone(),
                &control,
                Timestamp::now_utc().to_millis(),
            )
            .await?;
        }
        Ok(mapping)
    }

    pub(crate) async fn attach_foreign_handle(
        &self,
        handle: DurableStreamHandleV1,
        role: SessionStreamRoleV1,
        epoch: u64,
    ) -> Result<StreamSessionMappingRecordV1, String> {
        if self.producer.owns_handle_identity(&handle) {
            return Err("attached durable stream handle is owned by the consumer".to_string());
        }
        if let Some(mapping) = self.mapping_for_handle(&handle, role) {
            return Ok(mapping);
        }
        let mapping = StreamSessionMappingRecordV1 {
            transport_stream_id: self.allocate_transport_stream_id()?,
            handle,
            role,
        };
        self.activate_foreign_mapping(mapping.clone(), epoch)
            .await?;
        Ok(mapping)
    }

    pub(crate) async fn write_input(
        &self,
        transport_stream_id: u64,
        first_sequence: u64,
        payload: StreamItemsPayloadV1,
    ) -> Result<
        (
            u64,
            u64,
            golem_common::model::durable_stream::StreamOffsetV1,
            Vec<StreamSessionMappingRecordV1>,
        ),
        String,
    > {
        let _session_guard = self.session_lock.lock().await;
        self.ensure_current_attachment().await?;
        let handle = self
            .handle(transport_stream_id)
            .ok_or_else(|| format!("unknown durable input stream {transport_stream_id}"))?;
        let logical_item_count = u64::try_from(payload.logical_item_count())
            .map_err(|_| "durable input item count does not fit in u64".to_string())?;
        let mut nested_transport_ids = Vec::new();
        let mut nested_requests = Vec::new();
        let mut nested_element_types = Vec::new();
        if let StreamItemsPayloadV1::Values(values) = &payload {
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
                    let coordinate = StreamRegistrationCoordinateV1::Nested {
                        parent_stream_id: handle.stream_id,
                        parent_producer_sequence,
                        recursive_value_path: path,
                    };
                    if let Some(existing) = self.handle(nested_transport_id)
                        && self
                            .producer
                            .handle_for_coordinate(&coordinate)
                            .await
                            .as_ref()
                            != Some(&existing)
                    {
                        return Err(format!(
                            "nested durable transport stream id {nested_transport_id} conflicts with its persisted coordinate"
                        ));
                    }
                    nested_transport_ids.push(nested_transport_id);
                    nested_element_types
                        .push((nested_transport_id, element.unwrap_or_else(SchemaType::u8)));
                    nested_requests.push(ProducerRegistrationRequestV1 {
                        coordinate,
                        source_invocation: self.session_key.clone(),
                        component_revision: input_schema.component_revision,
                        element_schema_fingerprint,
                        source_kind: StreamSourceKindV1::Nested,
                        session_mapping: None,
                    });
                }
            }
        }
        let canonical_payload = match &payload {
            StreamItemsPayloadV1::Values(values) => {
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
                StreamItemsPayloadV1::Values(canonical_values)
            }
            StreamItemsPayloadV1::PackedU8(bytes) => StreamItemsPayloadV1::PackedU8(bytes.clone()),
        };
        let outcome = self
            .producer
            .write_items_with_nested(
                handle.stream_id,
                first_sequence,
                canonical_payload,
                nested_requests,
            )
            .await
            .map_err(|error| error.to_string())?;
        let mut nested_mappings = Vec::with_capacity(nested_transport_ids.len());
        if !nested_transport_ids.is_empty() {
            let nested_handles = self
                .producer
                .nested_handles(handle.stream_id, first_sequence)
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
                self.insert_mapping(
                    transport_stream_id,
                    handle.clone(),
                    SessionStreamRoleV1::Input,
                )?;
                nested_mappings.push(StreamSessionMappingRecordV1 {
                    transport_stream_id,
                    handle,
                    role: SessionStreamRoleV1::Input,
                });
            }
            for mapping in &nested_mappings {
                self.append_mapping_once(mapping.clone()).await?;
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
        Ok((
            highest_contiguous_sequence,
            logical_item_count,
            resulting_offset,
            nested_mappings,
        ))
    }

    pub(crate) async fn end_input(
        &self,
        transport_stream_id: u64,
        sequence: u64,
    ) -> Result<golem_common::model::durable_stream::StreamOffsetV1, String> {
        let _session_guard = self.session_lock.lock().await;
        self.ensure_current_attachment().await?;
        let handle = self
            .handle(transport_stream_id)
            .ok_or_else(|| format!("unknown durable input stream {transport_stream_id}"))?;
        self.producer
            .end(handle.stream_id, sequence, StreamEndResultV1::Ok)
            .await
            .map(|outcome| outcome.value)
            .map_err(|error| error.to_string())
    }

    pub(crate) async fn cancel_stream(
        &self,
        transport_stream_id: u64,
        role: StreamCancelRoleV1,
        reason: StreamCancelReasonV1,
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
            StreamCancelRoleV1::InputProducer | StreamCancelRoleV1::InputConsumer => {
                SessionStreamRoleV1::Input
            }
            StreamCancelRoleV1::OutputProducer | StreamCancelRoleV1::OutputConsumer => {
                SessionStreamRoleV1::Output
            }
            StreamCancelRoleV1::System => {
                return Err("system-authored durable stream cancellation is internal".to_string());
            }
        };
        if mapping.role != expected_role {
            return Err("durable stream cancellation role does not match its mapping".to_string());
        }
        let (epoch, _, _) = self.authoritative_attachment_state().await?;
        let intent = StreamConsumerCancelIntentRecordV1 {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            session_key: self.session_key.clone(),
            stream_id: mapping.handle.stream_id,
            epoch,
            role,
            reason,
            details,
        };
        let current = self.oplog.current_oplog_index().await;
        let mut persisted_intent = None;
        if current.is_defined() {
            for (_, entry) in self
                .oplog
                .read_many(
                    golem_common::model::oplog::OplogIndex::INITIAL,
                    current.as_u64(),
                )
                .await
            {
                let OplogEntry::StreamSession { record, .. } = entry else {
                    continue;
                };
                if let StreamSessionRecordV1::ConsumerCancelIntent(existing) =
                    self.download_record(record).await?
                    && existing.session_key == self.session_key
                    && existing.stream_id == mapping.handle.stream_id
                {
                    persisted_intent = Some(existing);
                    break;
                }
            }
        }
        let intent = match persisted_intent {
            Some(existing) => existing,
            None => {
                self.append_record(StreamSessionRecordV1::ConsumerCancelIntent(intent.clone()))
                    .await;
                self.commit_consumer_journal().await?;
                intent
            }
        };
        drop(session_guard);
        if self.producer.owns_handle_identity(&mapping.handle) {
            self.producer
                .cancel_open(
                    mapping.handle.stream_id,
                    intent.role,
                    intent.reason,
                    intent.details,
                )
                .await
                .map_err(|error| error.to_string())?;
        } else {
            let rpc = self.rpc.clone().ok_or_else(|| {
                "foreign durable stream cancellation routing is unavailable".to_string()
            })?;
            let auth_ctx = self.auth_ctx.clone().ok_or_else(|| {
                "foreign durable stream cancellation authorization is unavailable".to_string()
            })?;
            RoutedStreamAttachmentControl::new(rpc, mapping.clone(), auth_ctx)
                .cancel_stream(
                    self.attachment_key(&mapping.handle, intent.epoch)?,
                    intent.role,
                    intent.reason,
                    intent.details,
                )
                .await
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub(crate) async fn input_high_waters(
        &self,
    ) -> Result<HashMap<u64, InputStreamHighWaterV1>, String> {
        let mappings = self
            .mappings
            .read()
            .expect("durable stream mapping lock poisoned")
            .clone();
        let mut result = HashMap::new();
        for (transport_stream_id, (handle, role)) in mappings {
            if role != SessionStreamRoleV1::Input || !self.producer.owns_handle_identity(&handle) {
                continue;
            }
            if let Some(high_water) = self
                .producer
                .input_high_water(handle.stream_id)
                .await
                .map_err(|error| error.to_string())?
            {
                result.insert(transport_stream_id, high_water);
            }
        }
        Ok(result)
    }

    pub(crate) async fn materialize_agent_input(
        &self,
        value: &SchemaValue,
        graph: &SchemaGraph,
        root: &SchemaType,
        component_revision: golem_common::model::component::ComponentRevision,
    ) -> Result<(ProtoSchemaValue, Vec<StreamSessionMappingRecordV1>), String> {
        preflight_recursive_stream_value(value)?;
        struct PendingInput {
            path: Vec<StreamValuePathStepV1>,
            endpoint: Option<LiveStreamEndpoint>,
            forwarded_handle: Option<DurableStreamHandleV1>,
            element_type: SchemaType,
            element_schema_fingerprint: SchemaFingerprintV1,
        }

        let _session_guard = self.session_lock.lock().await;
        self.recover_session_mappings().await?;
        let mut pending = Vec::new();
        let encoded =
            encode_recursive_stream_value_with_schema(value, graph, root, |stream, path| {
                let element = stream_element_schema(graph, root, path)?;
                let element_schema_fingerprint =
                    schema_fingerprint_v1(graph, element).map_err(|error| error.to_string())?;
                let forwarded_handle = stream
                    .with_host_endpoint::<ForwardedDurableInput, _>(|forwarded| {
                        forwarded.handle.clone()
                    })
                    .ok();
                if let Some(handle) = &forwarded_handle
                    && (handle.format_version != DURABLE_STREAM_FORMAT_VERSION
                        || handle.element_schema_fingerprint != element_schema_fingerprint)
                {
                    return Err(
                    "forwarded durable input handle does not match the invocation stream schema"
                        .to_string(),
                );
                }
                pending.push(PendingInput {
                    path: path.to_vec(),
                    endpoint: if forwarded_handle.is_none() {
                        Some(stream.take_host_endpoint::<LiveStreamEndpoint>()?)
                    } else {
                        stream.take_host_endpoint::<ForwardedDurableInput>()?;
                        None
                    },
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

        let session_mapping = StreamSessionMappingV1 {
            session_key: self.session_key.clone(),
            attachment_id: golem_common::model::durable_stream::AttachmentId::primary(
                self.session_key.callee_environment_id,
                &self.session_key.callee,
                &self.session_key.idempotency_key,
            )
            .map_err(|error| error.to_string())?,
            role: SessionStreamRoleV1::Input,
        };
        let mut mappings = Vec::with_capacity(pending.len());
        let mut drains = Vec::with_capacity(pending.len());
        for (transport_stream_id, pending) in pending.into_iter().enumerate() {
            let transport_stream_id = u64::try_from(transport_stream_id)
                .map_err(|_| "durable input transport stream id overflow".to_string())?;
            let handle = if let Some(handle) = pending.forwarded_handle {
                handle
            } else {
                let request = ProducerRegistrationRequestV1 {
                    coordinate: StreamRegistrationCoordinateV1::Root {
                        invocation_id: self.session_key.clone(),
                        root_kind: StreamRootKindV1::MethodInput,
                        recursive_value_path: pending.path,
                    },
                    source_invocation: self.session_key.clone(),
                    component_revision,
                    element_schema_fingerprint: pending.element_schema_fingerprint,
                    source_kind: StreamSourceKindV1::AgentHostedInput,
                    session_mapping: Some(session_mapping.clone()),
                };
                self.producer
                    .register(request)
                    .await
                    .map_err(|error| error.to_string())?
                    .value
            };
            let mapping = StreamSessionMappingRecordV1 {
                transport_stream_id,
                handle: handle.clone(),
                role: SessionStreamRoleV1::Input,
            };
            if self.producer.owns_handle_identity(&handle) {
                self.append_mapping_once(mapping.clone()).await?;
            }
            self.insert_mapping(
                transport_stream_id,
                handle.clone(),
                SessionStreamRoleV1::Input,
            )?;
            mappings.push(mapping);
            if let Some(endpoint) = pending.endpoint {
                drains.push(PendingOwnedStreamDrain {
                    handle,
                    endpoint,
                    element_type: pending.element_type,
                    role: SessionStreamRoleV1::Input,
                });
            }
        }
        if !mappings.is_empty() {
            self.commit_consumer_journal().await?;
        }
        drop(_session_guard);

        if !drains.is_empty() {
            let streams = self.clone();
            let graph = Arc::new(graph.clone());
            tokio::spawn(async move {
                let (nested_tx, mut nested_rx) = mpsc::unbounded_channel();
                let mut tasks = tokio::task::JoinSet::new();
                for drain in drains {
                    let streams = streams.clone();
                    let graph = graph.clone();
                    let nested_tx = nested_tx.clone();
                    tasks.spawn(async move { streams.drain_output(drain, graph, nested_tx).await });
                }
                while !tasks.is_empty() {
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

    pub(crate) async fn materialize_result(
        &self,
        value: SchemaValue,
        graph: &SchemaGraph,
        root: &SchemaType,
        component_revision: golem_common::model::component::ComponentRevision,
    ) -> Result<SchemaValue, String> {
        preflight_recursive_stream_value(&value)?;
        let session_guard = self.session_lock.lock().await;
        struct PendingOutput {
            path: Vec<StreamValuePathStepV1>,
            endpoint: Option<LiveStreamEndpoint>,
            forwarded_handle: Option<DurableStreamHandleV1>,
            element_type: SchemaType,
            element_schema_fingerprint: SchemaFingerprintV1,
        }

        let mut pending = Vec::new();
        let encoded =
            encode_recursive_stream_value_with_schema(&value, graph, root, |stream, path| {
                let element = stream_element_schema(graph, root, path)?;
                let element_schema_fingerprint =
                    schema_fingerprint_v1(graph, element).map_err(|error| error.to_string())?;
                let forwarded_handle = stream
                    .with_host_endpoint::<ForwardedDurableInput, _>(|forwarded| {
                        forwarded.handle.clone()
                    })
                    .ok();
                if let Some(handle) = &forwarded_handle
                    && (handle.format_version != DURABLE_STREAM_FORMAT_VERSION
                        || handle.element_schema_fingerprint != element_schema_fingerprint)
                {
                    return Err(
                        "forwarded durable output handle does not match the result stream schema"
                            .to_string(),
                    );
                }
                let endpoint = if forwarded_handle.is_none() {
                    Some(stream.take_host_endpoint::<LiveStreamEndpoint>()?)
                } else {
                    stream.take_host_endpoint::<ForwardedDurableInput>()?;
                    None
                };
                let canonical_handle_index = u64::try_from(pending.len())
                    .map_err(|_| "durable output handle index overflow".to_string())?;
                pending.push(PendingOutput {
                    path: path.to_vec(),
                    endpoint,
                    forwarded_handle,
                    element_type: element.cloned().unwrap_or_else(SchemaType::u8),
                    element_schema_fingerprint,
                });
                Ok(canonical_handle_index)
            })?;

        self.producer
            .validate_new_session_stream_count(&self.session_key, pending.len())
            .await
            .map_err(|error| error.to_string())?;

        let session_mapping = StreamSessionMappingV1 {
            session_key: self.session_key.clone(),
            attachment_id: golem_common::model::durable_stream::AttachmentId::primary(
                self.session_key.callee_environment_id,
                &self.session_key.callee,
                &self.session_key.idempotency_key,
            )
            .map_err(|error| error.to_string())?,
            role: golem_common::model::durable_stream::SessionStreamRoleV1::Output,
        };
        let requests = pending
            .iter()
            .filter(|pending| pending.forwarded_handle.is_none())
            .map(|pending| ProducerRegistrationRequestV1 {
                coordinate: StreamRegistrationCoordinateV1::Root {
                    invocation_id: self.session_key.clone(),
                    root_kind: StreamRootKindV1::MethodResult,
                    recursive_value_path: pending.path.clone(),
                },
                source_kind: StreamSourceKindV1::InvocationOutput,
                source_invocation: self.session_key.clone(),
                component_revision,
                element_schema_fingerprint: pending.element_schema_fingerprint.clone(),
                session_mapping: Some(session_mapping.clone()),
            })
            .collect::<Vec<_>>();
        self.recover_session_mappings().await?;
        let mut transport_stream_ids = Vec::with_capacity(pending.len());
        let mut request_index = 0usize;
        for pending in &pending {
            let transport_stream_id = if let Some(handle) = &pending.forwarded_handle {
                self.ensure_nested_mapping_under_lock(handle.clone(), SessionStreamRoleV1::Output)
                    .await?
                    .transport_stream_id
            } else {
                let request = &requests[request_index];
                request_index += 1;
                let existing_mapping = self
                    .producer
                    .handle_for_coordinate(&request.coordinate)
                    .await
                    .and_then(|handle| {
                        self.mapping_for_handle(&handle, SessionStreamRoleV1::Output)
                    });
                existing_mapping
                    .map(|mapping| mapping.transport_stream_id)
                    .map(Ok)
                    .unwrap_or_else(|| self.allocate_transport_stream_id())?
            };
            transport_stream_ids.push(transport_stream_id);
        }
        let result_bytes = encoded.encode_to_vec();
        let result_session_key = self.session_key.clone();
        let transport_stream_ids_for_record = transport_stream_ids.clone();
        let forwarded_handles_for_record = pending
            .iter()
            .map(|pending| pending.forwarded_handle.clone())
            .collect::<Vec<_>>();
        let (owned_handles, _) = self
            .producer
            .register_result_streams(requests, move |owned_handles| {
                let mut owned_handles = owned_handles.into_iter();
                let handles = forwarded_handles_for_record
                    .into_iter()
                    .map(|forwarded_handle| {
                        forwarded_handle.unwrap_or_else(|| {
                            owned_handles
                                .next()
                                .expect("result registration returned too few durable handles")
                        })
                    })
                    .collect::<Vec<_>>();
                let stream_mappings = transport_stream_ids_for_record
                    .into_iter()
                    .zip(handles.iter().cloned())
                    .map(
                        |(transport_stream_id, handle)| StreamSessionMappingRecordV1 {
                            transport_stream_id,
                            handle,
                            role: golem_common::model::durable_stream::SessionStreamRoleV1::Output,
                        },
                    )
                    .collect();
                StreamSessionRecordV1::InvocationResult(StreamSessionInvocationResultRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: result_session_key,
                    result: result_bytes,
                    output_streams: handles,
                    stream_mappings,
                })
            })
            .await
            .map_err(|error| error.to_string())?;
        self.producer.notify_session_records_changed();

        let mut drains = Vec::with_capacity(pending.len());
        let mut owned_handles = owned_handles.into_iter();
        for (pending, transport_stream_id) in pending.into_iter().zip(transport_stream_ids) {
            let handle = pending.forwarded_handle.unwrap_or_else(|| {
                owned_handles
                    .next()
                    .expect("result registration returned too few durable handles")
            });
            self.insert_mapping(
                transport_stream_id,
                handle.clone(),
                SessionStreamRoleV1::Output,
            )?;
            if let Some(endpoint) = pending.endpoint {
                drains.push(PendingOwnedStreamDrain {
                    handle,
                    endpoint,
                    element_type: pending.element_type,
                    role: SessionStreamRoleV1::Output,
                });
            }
        }
        drop(session_guard);

        if !drains.is_empty() {
            let graph = Arc::new(graph.clone());
            let (nested_tx, mut nested_rx) = mpsc::unbounded_channel();
            let mut tasks = tokio::task::JoinSet::new();
            for drain in drains {
                let streams = self.clone();
                let graph = graph.clone();
                let nested_tx = nested_tx.clone();
                tasks.spawn(async move { streams.drain_output(drain, graph, nested_tx).await });
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
        Ok(strip_streams(value))
    }

    pub(crate) async fn materialize_remote_result(
        &self,
        value: ProtoSchemaValue,
        remote_mappings: Vec<StreamSessionMappingRecordV1>,
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
            .any(|mapping| mapping.role != SessionStreamRoleV1::Output)
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
                self.mapping_for_handle(&mapping.handle, SessionStreamRoleV1::Output)
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
                    self.mapping_for_handle(&remote.handle, SessionStreamRoleV1::Output)
                {
                    mapping
                } else {
                    let mapping = StreamSessionMappingRecordV1 {
                        transport_stream_id: self.allocate_transport_stream_id()?,
                        handle: remote.handle,
                        role: SessionStreamRoleV1::Output,
                    };
                    self.insert_mapping(
                        mapping.transport_stream_id,
                        mapping.handle.clone(),
                        mapping.role,
                    )?;
                    mapping
                }
            } else {
                self.attach_foreign_handle(remote.handle, SessionStreamRoleV1::Output, 1)
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
        let record = StreamSessionInvocationResultRecordV1 {
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
            self.append_record(StreamSessionRecordV1::InvocationResult(record))
                .await;
            self.commit_consumer_journal().await?;
        }
        self.decode_initial(
            canonical,
            &mappings
                .iter()
                .map(|mapping| mapping.handle.clone())
                .collect::<Vec<_>>(),
            SessionStreamRoleV1::Output,
        )
        .await
    }

    pub(crate) async fn replay_remote_result(&self) -> Result<Option<SchemaValue>, String> {
        self.recover_session_mappings().await?;
        let Some(record) = self.remote_result_record().await? else {
            return Ok(None);
        };
        let value = ProtoSchemaValue::decode(record.result.as_slice())
            .map_err(|error| format!("invalid durable caller result: {error}"))?;
        self.decode_initial(value, &record.output_streams, SessionStreamRoleV1::Output)
            .await
            .map(Some)
    }

    async fn remote_result_record(
        &self,
    ) -> Result<Option<StreamSessionInvocationResultRecordV1>, String> {
        let current = self.oplog.current_oplog_index().await;
        if !current.is_defined() {
            return Ok(None);
        }
        for (_, entry) in self
            .oplog
            .read_many(
                golem_common::model::oplog::OplogIndex::INITIAL,
                current.as_u64(),
            )
            .await
        {
            let OplogEntry::StreamSession { record, .. } = entry else {
                continue;
            };
            if let StreamSessionRecordV1::InvocationResult(record) =
                self.download_record(record).await?
                && record.session_key == self.session_key
            {
                return Ok(Some(record));
            }
        }
        Ok(None)
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
        if self.require_attachment_before_production {
            self.wait_for_active_attachment(&handle).await?;
        }
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
        loop {
            let received = tokio::select! {
                biased;
                _ = source_cancelled.cancelled() => {
                    tracing::debug!(
                        stream_id = %handle.stream_id,
                        role = ?role,
                        "Durable stream output drain stopped after consumer cancellation"
                    );
                    break;
                },
                _ = lifecycle.cancelled() => {
                    tracing::debug!(
                        stream_id = %handle.stream_id,
                        role = ?role,
                        "Durable stream output drain stopped after runtime teardown"
                    );
                    break;
                },
                received = source.recv() => received,
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
                            handle.stream_id,
                            next_sequence,
                            StreamEndResultV1::ErrorContext(format!("{error:?}").into_bytes()),
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
                        forwarded_handle: Option<DurableStreamHandleV1>,
                        element_type: SchemaType,
                        registration: ProducerRegistrationRequestV1,
                    }

                    if let Err(error) = preflight_recursive_stream_value(&value) {
                        let _ = self
                            .producer
                            .end(
                                handle.stream_id,
                                event.offset,
                                StreamEndResultV1::ErrorContext(error.into_bytes()),
                            )
                            .await;
                        break;
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
                            let forwarded_handle = stream
                                .with_host_endpoint::<ForwardedDurableInput, _>(|forwarded| {
                                    forwarded.handle.clone()
                                })
                                .ok();
                            if let Some(forwarded_handle) = &forwarded_handle
                                && (forwarded_handle.format_version
                                    != DURABLE_STREAM_FORMAT_VERSION
                                    || forwarded_handle.element_schema_fingerprint
                                        != element_schema_fingerprint)
                            {
                                return Err(
                                "forwarded nested durable handle does not match the stream item schema"
                                    .to_string(),
                            );
                            }
                            let endpoint = if forwarded_handle.is_none() {
                                Some(stream.take_host_endpoint::<LiveStreamEndpoint>()?)
                            } else {
                                stream.take_host_endpoint::<ForwardedDurableInput>()?;
                                None
                            };
                            let canonical_handle_index = u64::try_from(nested_outputs.len())
                                .map_err(|_| "durable nested handle index overflow".to_string())?;
                            nested_outputs.push(NestedOutput {
                                endpoint,
                                forwarded_handle,
                                element_type: nested_element.unwrap_or_else(SchemaType::u8),
                                registration: ProducerRegistrationRequestV1 {
                                    coordinate: StreamRegistrationCoordinateV1::Nested {
                                        parent_stream_id: handle.stream_id,
                                        parent_producer_sequence: event.offset,
                                        recursive_value_path: path.to_vec(),
                                    },
                                    source_invocation: self.session_key.clone(),
                                    component_revision: handle.component_revision,
                                    element_schema_fingerprint,
                                    source_kind: StreamSourceKindV1::Nested,
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
                                    handle.stream_id,
                                    event.offset,
                                    StreamEndResultV1::ErrorContext(error.into_bytes()),
                                )
                                .await;
                            break;
                        }
                    };
                    for output in &nested_outputs {
                        if let Some(forwarded_handle) = &output.forwarded_handle {
                            self.ensure_nested_mapping(forwarded_handle.clone(), role)
                                .await?;
                        }
                    }
                    let nested_sources = nested_outputs
                        .iter()
                        .map(|output| {
                            output
                                .forwarded_handle
                                .clone()
                                .map(NestedStreamWriteV1::Forward)
                                .unwrap_or_else(|| {
                                    NestedStreamWriteV1::Register(output.registration.clone())
                                })
                        })
                        .collect();
                    match self
                        .producer
                        .write_items_with_nested_sources(
                            handle.stream_id,
                            event.offset,
                            StreamItemsPayloadV1::Values(vec![value.encode_to_vec()]),
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
                            let nested_handles = self
                                .producer
                                .nested_handles(handle.stream_id, event.offset)
                                .await
                                .map_err(|error| error.to_string())?;
                            if nested_handles.len() != nested_outputs.len() {
                                return Err("nested output stream mapping count does not match durable item metadata".to_string());
                            }
                            if !nested_outputs.is_empty() {
                                let _session_guard = self.session_lock.lock().await;
                                for (output, nested_handle) in
                                    nested_outputs.into_iter().zip(nested_handles)
                                {
                                    let transport_stream_id = self
                                        .mapping_for_handle(&nested_handle, role)
                                        .map(|mapping| mapping.transport_stream_id)
                                        .map(Ok)
                                        .unwrap_or_else(|| self.allocate_transport_stream_id())?;
                                    self.insert_mapping(
                                        transport_stream_id,
                                        nested_handle.clone(),
                                        role,
                                    )?;
                                    self.append_mapping_once(StreamSessionMappingRecordV1 {
                                        transport_stream_id,
                                        handle: nested_handle.clone(),
                                        role,
                                    })
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
                            }
                            Ok(())
                        }
                        Err(error) => Err(error.to_string()),
                    }
                }
                LiveStreamEventPayload::End => self
                    .producer
                    .end(handle.stream_id, event.offset, StreamEndResultV1::Ok)
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
                LiveStreamEventPayload::Error(error) => self
                    .producer
                    .end(
                        handle.stream_id,
                        event.offset,
                        StreamEndResultV1::ErrorContext(error.into_bytes()),
                    )
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
            };
            if let Err(error) = result {
                let _ = self
                    .producer
                    .end_open(
                        handle.stream_id,
                        StreamEndResultV1::ErrorContext(error.into_bytes()),
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

    async fn wait_for_active_attachment(
        &self,
        handle: &DurableStreamHandleV1,
    ) -> Result<(), String> {
        loop {
            let changed = self.producer.session_records_changed().notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let active = StreamAttachmentControl::inspect_attachments(self.producer.as_ref())
                .await
                .into_iter()
                .any(|attachment| {
                    attachment.key.session_key == self.session_key
                        && attachment.key.stream_id == handle.stream_id
                        && attachment.key.producer_environment_id == handle.producer_environment_id
                        && attachment.key.producer == handle.producer
                        && attachment.key.expected_producer_fingerprint
                            == handle.expected_producer_fingerprint
                        && attachment.state == StreamAttachmentStateV1::Active
                });
            if active {
                return Ok(());
            }
            changed.await;
        }
    }

    async fn finish(
        &self,
        result: Result<(), Vec<u8>>,
        input_cancel_reason: StreamCancelReasonV1,
    ) -> Result<(), String> {
        let session_guard = self.session_lock.lock().await;
        self.validate_topology_complete().await?;
        drop(session_guard);
        self.producer
            .finish_session(self.session_key.clone(), result, input_cancel_reason)
            .await
            .map_err(|error| error.to_string())
    }

    async fn validate_topology_complete(&self) -> Result<(), String> {
        let current = self.oplog.current_oplog_index().await;
        if !current.is_defined() {
            return Ok(());
        }
        let mut topology = HashMap::<
            (
                golem_common::base_model::durable_stream::AttachmentId,
                golem_common::base_model::durable_stream::StreamId,
                u64,
                SessionStreamRoleV1,
            ),
            (
                StreamAttachmentKeyV1,
                StreamSessionMappingRecordV1,
                ConsumerAttachmentStatus,
            ),
        >::new();
        let mut visible_mappings = Vec::new();
        for (_, entry) in self
            .oplog
            .read_many(
                golem_common::model::oplog::OplogIndex::INITIAL,
                current.as_u64(),
            )
            .await
        {
            let OplogEntry::StreamSession { record, .. } = entry else {
                continue;
            };
            match self.download_record(record).await? {
                StreamSessionRecordV1::Prepared(record)
                    if record.attempt.session_key == self.session_key =>
                {
                    visible_mappings.extend(record.stream_mappings);
                }
                StreamSessionRecordV1::TopologyPrepared(record)
                    if record.session_key == self.session_key =>
                {
                    let slot = (
                        record.attachment.attachment_id,
                        record.attachment.stream_id,
                        record.mapping.transport_stream_id,
                        record.mapping.role,
                    );
                    match topology.get(&slot) {
                        Some((attachment, mapping, _))
                            if attachment != &record.attachment || mapping != &record.mapping =>
                        {
                            return Err("conflicting durable topology preparation".to_string());
                        }
                        Some(_) => {}
                        None => {
                            topology.insert(
                                slot,
                                (
                                    record.attachment,
                                    record.mapping,
                                    ConsumerAttachmentStatus::Prepared,
                                ),
                            );
                        }
                    }
                }
                StreamSessionRecordV1::TopologyActivated(record)
                    if record.session_key == self.session_key =>
                {
                    let slot = (
                        record.attachment.attachment_id,
                        record.attachment.stream_id,
                        record.mapping.transport_stream_id,
                        record.mapping.role,
                    );
                    let Some((attachment, mapping, state)) = topology.get_mut(&slot) else {
                        return Err(
                            "durable topology activation has no matching preparation".to_string()
                        );
                    };
                    if attachment != &record.attachment || mapping != &record.mapping {
                        return Err("conflicting durable topology activation".to_string());
                    }
                    *state = ConsumerAttachmentStatus::Active;
                }
                StreamSessionRecordV1::Mapping(record)
                    if record.session_key == self.session_key =>
                {
                    visible_mappings.push(record.mapping);
                }
                StreamSessionRecordV1::InvocationResult(record)
                    if record.session_key == self.session_key =>
                {
                    visible_mappings.extend(record.stream_mappings);
                }
                _ => {}
            }
        }
        for (_, mapping, state) in topology.values() {
            if *state != ConsumerAttachmentStatus::Active {
                return Err(
                    "durable session has prepared but inactive foreign topology".to_string()
                );
            }
            if !visible_mappings.contains(mapping) {
                return Err(
                    "durable session has activated foreign topology without a visible mapping"
                        .to_string(),
                );
            }
        }
        Ok(())
    }

    pub(crate) async fn fail(&self, details: String) -> Result<(), String> {
        self.finish(
            Err(details.into_bytes()),
            StreamCancelReasonV1::InvocationFailed,
        )
        .await
    }

    pub(crate) async fn fail_invocation(&self, details: String) -> Result<(), String> {
        self.finish(
            Err(details.into_bytes()),
            StreamCancelReasonV1::InvocationFailed,
        )
        .await
    }

    pub(crate) async fn fail_protocol(&self, details: String) -> Result<(), String> {
        self.finish(Err(details.into_bytes()), StreamCancelReasonV1::Protocol)
            .await
    }

    pub(crate) async fn complete(&self) -> Result<(), String> {
        self.finish(Ok(()), StreamCancelReasonV1::GuestDrop).await
    }

    pub(crate) async fn persisted_result(
        &self,
    ) -> Result<Option<(ProtoSchemaValue, Vec<DurableStreamMapping>)>, String> {
        let current = self.oplog.current_oplog_index().await;
        if !current.is_defined() {
            return Ok(None);
        }
        for (_, entry) in self
            .oplog
            .read_many(
                golem_common::model::oplog::OplogIndex::INITIAL,
                current.as_u64(),
            )
            .await
        {
            let OplogEntry::StreamSession { record, .. } = entry else {
                continue;
            };
            let record = self.download_record(record).await?;
            if let StreamSessionRecordV1::InvocationResult(result) = record
                && result.session_key == self.session_key
            {
                let value =
                    ProtoSchemaValue::decode(result.result.as_slice()).map_err(|error| {
                        format!("invalid persisted durable invocation result: {error}")
                    })?;
                let transport_value =
                    remap_recursive_stream_references(value, |handle_index, _| {
                        let index = usize::try_from(handle_index).map_err(|_| {
                            format!("durable result handle index {handle_index} is too large")
                        })?;
                        result
                            .stream_mappings
                            .get(index)
                            .map(|mapping| mapping.transport_stream_id)
                            .ok_or_else(|| {
                                format!("unknown durable result handle index {handle_index}")
                            })
                    })?;
                let proto_mappings = result
                    .stream_mappings
                    .iter()
                    .map(|mapping| durable_stream_mapping_to_proto(mapping, None))
                    .collect();
                for mapping in result.stream_mappings {
                    self.insert_mapping(mapping.transport_stream_id, mapping.handle, mapping.role)?;
                }
                return Ok(Some((transport_value, proto_mappings)));
            }
        }
        Ok(None)
    }

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

    pub(crate) async fn persisted_finished(&self) -> Result<Option<Result<(), Vec<u8>>>, String> {
        let current = self.oplog.current_oplog_index().await;
        if !current.is_defined() {
            return Ok(None);
        }
        for (_, entry) in self
            .oplog
            .read_many(
                golem_common::model::oplog::OplogIndex::INITIAL,
                current.as_u64(),
            )
            .await
        {
            let OplogEntry::StreamSession { record, .. } = entry else {
                continue;
            };
            if let StreamSessionRecordV1::Finished(record) = self.download_record(record).await?
                && record.session_key == self.session_key
            {
                return Ok(Some(record.result));
            }
        }
        Ok(None)
    }

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

    pub(crate) async fn pump_output_streams(
        &self,
        responses: &mpsc::Sender<InvocationResponse>,
    ) -> Result<(), String> {
        self.recover_session_mappings().await?;
        let output_mapping_ids = self.session_root_output_mapping_ids().await?;
        self.pump_output_streams_from_recovered(&HashMap::new(), &output_mapping_ids, responses)
            .await
    }

    pub(crate) async fn pump_output_streams_from(
        &self,
        cursors: &HashMap<
            golem_common::model::durable_stream::StreamId,
            Option<golem_common::model::durable_stream::StreamOffsetV1>,
        >,
        output_mapping_ids: &[u64],
        responses: &mpsc::Sender<InvocationResponse>,
    ) -> Result<(), String> {
        self.recover_session_mappings().await?;
        self.pump_output_streams_from_recovered(cursors, output_mapping_ids, responses)
            .await
    }

    async fn pump_output_streams_from_recovered(
        &self,
        cursors: &HashMap<
            golem_common::model::durable_stream::StreamId,
            Option<golem_common::model::durable_stream::StreamOffsetV1>,
        >,
        output_mapping_ids: &[u64],
        responses: &mpsc::Sender<InvocationResponse>,
    ) -> Result<(), String> {
        let mut pending = output_mapping_ids
            .iter()
            .copied()
            .map(|transport_stream_id| {
                let mapping = self.mapping(transport_stream_id).ok_or_else(|| {
                    format!("unknown durable output stream {transport_stream_id}")
                })?;
                Ok((transport_stream_id, mapping.handle))
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut seen = pending
            .iter()
            .map(|(transport_stream_id, _)| *transport_stream_id)
            .collect::<HashSet<_>>();
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

    pub(crate) async fn pump_input_cancellations(
        &self,
        responses: &mpsc::Sender<InvocationResponse>,
    ) -> Result<(), String> {
        self.recover_session_mappings().await?;
        let inputs = self
            .mappings
            .read()
            .expect("durable stream mapping lock poisoned")
            .iter()
            .filter_map(|(transport_stream_id, (handle, role))| {
                (*role == SessionStreamRoleV1::Input
                    && handle.producer_environment_id == self.session_key.callee_environment_id
                    && handle.producer == self.session_key.callee
                    && handle.expected_producer_fingerprint == self.session_key.callee_fingerprint)
                    .then_some((*transport_stream_id, handle.clone()))
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
                    CommittedProducerStreamEventPayloadV1::Cancel {
                        role: StreamCancelRoleV1::InputConsumer,
                        reason,
                        details,
                    } => {
                        let reason = match reason {
                            StreamCancelReasonV1::Protocol => {
                                golem_api_grpc::proto::golem::worker::StreamCancelReason::Protocol
                            }
                            _ => {
                                golem_api_grpc::proto::golem::worker::StreamCancelReason::Cancelled
                            }
                        };
                        responses
                            .send(InvocationResponse {
                                response: Some(invocation_response::Response::StreamCancel(
                                    StreamCancel {
                                        transport_stream_id,
                                        producer_sequence: event.producer_sequence,
                                        role: golem_api_grpc::proto::golem::worker::StreamCancelRole::InputConsumer as i32,
                                        reason: reason as i32,
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
                    CommittedProducerStreamEventPayloadV1::End(_)
                    | CommittedProducerStreamEventPayloadV1::Cancel { .. } => break,
                    CommittedProducerStreamEventPayloadV1::Value(_)
                    | CommittedProducerStreamEventPayloadV1::PackedU8(_) => {}
                }
            }
        }
        Ok(())
    }

    async fn pump_output_stream_from(
        &self,
        transport_stream_id: u64,
        handle: DurableStreamHandleV1,
        after: Option<golem_common::model::durable_stream::StreamOffsetV1>,
        responses: &mpsc::Sender<InvocationResponse>,
    ) -> Result<Vec<(u64, DurableStreamHandleV1)>, String> {
        let durable_stream_id = handle.stream_id;
        let mut nested_streams = Vec::new();
        let mut reader = self
            .stream_reader(handle, after, SessionStreamRoleV1::Output)
            .await?;
        while let Some(event) = reader.next().await.map_err(|error| error.to_string())? {
            self.ensure_current_attachment().await?;
            let response = match event.payload {
                CommittedProducerStreamEventPayloadV1::Value(bytes) => {
                    let _session_guard = self.session_lock.lock().await;
                    self.recover_session_mappings().await?;
                    let value = ProtoSchemaValue::decode(bytes.as_slice())
                        .map_err(|error| format!("invalid durable output value: {error}"))?;
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
                        let mapping = match self
                            .mapping_for_handle(&handle, SessionStreamRoleV1::Output)
                        {
                            Some(mapping) => mapping,
                            None => {
                                let transport_stream_id = self.allocate_transport_stream_id()?;
                                let mapping = StreamSessionMappingRecordV1 {
                                    transport_stream_id,
                                    handle: handle.clone(),
                                    role: SessionStreamRoleV1::Output,
                                };
                                self.insert_mapping(
                                    transport_stream_id,
                                    handle,
                                    SessionStreamRoleV1::Output,
                                )?;
                                self.append_mapping_once(mapping.clone()).await?;
                                mapping
                            }
                        };
                        nested_streams.push((mapping.transport_stream_id, mapping.handle.clone()));
                        mappings.push(mapping);
                    }
                    if !mappings.is_empty() {
                        self.commit_consumer_journal().await?;
                    }
                    let value = remap_recursive_stream_references(value, |handle_index, _| {
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
                    invocation_response::Response::OutputItem(OutputStreamItem {
                        transport_stream_id,
                        producer_sequence: event.producer_sequence,
                        value: Some(value),
                        durable_stream_id: Some(durable_stream_id.0.into()),
                        durable_offset: event.offset.0.to_vec(),
                        epoch: self.attachment_epoch,
                        new_stream_mappings,
                    })
                }
                CommittedProducerStreamEventPayloadV1::PackedU8(value) => {
                    invocation_response::Response::OutputItem(OutputStreamItem {
                        transport_stream_id,
                        producer_sequence: event.producer_sequence,
                        value: Some(ProtoSchemaValue::try_from(SchemaValue::U8(value))?),
                        durable_stream_id: Some(durable_stream_id.0.into()),
                        durable_offset: event.offset.0.to_vec(),
                        epoch: self.attachment_epoch,
                        new_stream_mappings: Vec::new(),
                    })
                }
                CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::Ok) => {
                    invocation_response::Response::OutputEnd(OutputStreamEnd {
                        transport_stream_id,
                        producer_sequence: event.producer_sequence,
                        durable_stream_id: Some(durable_stream_id.0.into()),
                        durable_offset: event.offset.0.to_vec(),
                        epoch: self.attachment_epoch,
                    })
                }
                CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::ErrorContext(
                    details,
                )) => invocation_response::Response::OutputError(OutputStreamError {
                    transport_stream_id,
                    producer_sequence: event.producer_sequence,
                    details: String::from_utf8_lossy(&details).into_owned(),
                    durable_stream_id: Some(durable_stream_id.0.into()),
                    durable_offset: event.offset.0.to_vec(),
                    epoch: self.attachment_epoch,
                }),
                CommittedProducerStreamEventPayloadV1::Cancel {
                    role: _,
                    reason,
                    details,
                } => {
                    let reason = match reason {
                        StreamCancelReasonV1::Protocol => {
                            golem_api_grpc::proto::golem::worker::StreamCancelReason::Protocol
                        }
                        StreamCancelReasonV1::SourceUnavailable => {
                            golem_api_grpc::proto::golem::worker::StreamCancelReason::SourceUnavailable
                        }
                        StreamCancelReasonV1::ProducerDeleting => {
                            golem_api_grpc::proto::golem::worker::StreamCancelReason::ProducerDeleting
                        }
                        StreamCancelReasonV1::Cancelled
                        | StreamCancelReasonV1::GuestDrop
                        | StreamCancelReasonV1::InvocationFailed => {
                            golem_api_grpc::proto::golem::worker::StreamCancelReason::Cancelled
                        }
                    };
                    invocation_response::Response::StreamCancel(StreamCancel {
                        transport_stream_id,
                        producer_sequence: event.producer_sequence,
                        role: golem_api_grpc::proto::golem::worker::StreamCancelRole::OutputProducer
                            as i32,
                        reason: reason as i32,
                        details,
                        durable_stream_id: Some(durable_stream_id.0.into()),
                        epoch: self.attachment_epoch,
                        durable_offset: event.offset.0.to_vec(),
                    })
                }
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

    async fn session_root_output_mapping_ids(&self) -> Result<Vec<u64>, String> {
        let current = self.oplog.current_oplog_index().await;
        if !current.is_defined() {
            return Ok(Vec::new());
        }
        let mut output_ids = Vec::new();
        for (_, entry) in self
            .oplog
            .read_many(
                golem_common::model::oplog::OplogIndex::INITIAL,
                current.as_u64(),
            )
            .await
        {
            let OplogEntry::StreamSession { record, .. } = entry else {
                continue;
            };
            let record = self.download_record(record).await?;
            match record {
                StreamSessionRecordV1::InvocationResult(result)
                    if result.session_key == self.session_key =>
                {
                    output_ids.extend(result.stream_mappings.into_iter().filter_map(|mapping| {
                        (mapping.role
                            == golem_common::model::durable_stream::SessionStreamRoleV1::Output)
                            .then_some(mapping.transport_stream_id)
                    }));
                }
                _ => {}
            }
        }
        Ok(output_ids)
    }

    pub(crate) async fn decode_initial(
        &self,
        value: ProtoSchemaValue,
        handles: &[DurableStreamHandleV1],
        role: SessionStreamRoleV1,
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
        handle: DurableStreamHandleV1,
        consumer_read_ordinal: u64,
        role: SessionStreamRoleV1,
    ) -> Result<DurableInputEndpoint, String> {
        let (journal, after, _, terminal) = self.consumer_history(handle.stream_id).await?;
        let reader = if terminal {
            None
        } else {
            Some(self.stream_reader(handle.clone(), after, role).await?)
        };
        record_source_journal_lag(reader.as_ref(), after).await;
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
        handle: DurableStreamHandleV1,
        after: Option<golem_common::model::durable_stream::StreamOffsetV1>,
        role: SessionStreamRoleV1,
    ) -> Result<DurableStreamReader, String> {
        let reader = if self.producer.owns_handle_identity(&handle) {
            DurableStreamReader::Owned {
                reader: self
                    .producer
                    .catch_up(handle.clone(), after)
                    .await
                    .map_err(|error| error.to_string())?,
                source: self.producer.clone(),
                handle,
            }
        } else {
            let mapping = self
                .mapping_for_handle(&handle, role)
                .ok_or_else(|| "foreign durable stream has no session mapping".to_string())?;
            let attachment = self.attachment_key(&handle, 1)?;
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
            DurableStreamReader::Attached(AttachedDurableCatchUpReader {
                source: Arc::new(RoutedAttachedStreamSegmentSource::new(
                    rpc, mapping, auth_ctx,
                )),
                attachment,
                handle: handle.clone(),
                consumer_journal: self.consumer_journal.clone(),
                after,
                buffered: VecDeque::new(),
                terminal: false,
            })
        };
        Ok(reader)
    }

    async fn consumer_history(
        &self,
        stream_id: golem_common::model::durable_stream::StreamId,
    ) -> Result<
        (
            VecDeque<CommittedProducerStreamEventV1>,
            Option<golem_common::model::durable_stream::StreamOffsetV1>,
            u64,
            bool,
        ),
        String,
    > {
        let current = self.oplog.current_oplog_index().await;
        if !current.is_defined() {
            return Ok((VecDeque::new(), None, 0, false));
        }
        let mut events = Vec::new();
        for (_, entry) in self
            .oplog
            .read_many(
                golem_common::model::oplog::OplogIndex::INITIAL,
                current.as_u64(),
            )
            .await
        {
            let OplogEntry::StreamSession { record, .. } = entry else {
                continue;
            };
            let record = self.download_record(record).await?;
            match record {
                StreamSessionRecordV1::ConsumerItemValue(record)
                    if record.session_key == self.session_key && record.stream_id == stream_id =>
                {
                    for mapping in record.recursive_mappings {
                        self.insert_mapping(
                            mapping.transport_stream_id,
                            mapping.handle,
                            mapping.role,
                        )?;
                    }
                    events.push((
                        record.consumer_read_ordinal,
                        CommittedProducerStreamEventV1 {
                            stream_id,
                            producer_sequence: record.consumer_read_ordinal,
                            offset: record.source_offset,
                            terminal_author: None,
                            nested_handles: record.recursive_handles,
                            payload: if record.packed_u8 {
                                CommittedProducerStreamEventPayloadV1::PackedU8(
                                    *record.value.first().ok_or_else(|| {
                                        "packed-u8 consumer journal value is empty".to_string()
                                    })?,
                                )
                            } else {
                                CommittedProducerStreamEventPayloadV1::Value(record.value)
                            },
                        },
                    ));
                }
                StreamSessionRecordV1::ConsumerTerminal(record)
                    if record.session_key == self.session_key && record.stream_id == stream_id =>
                {
                    events.push((
                        record.consumer_read_ordinal,
                        CommittedProducerStreamEventV1 {
                            stream_id,
                            producer_sequence: record.consumer_read_ordinal,
                            offset: record.source_offset,
                            terminal_author: None,
                            nested_handles: Vec::new(),
                            payload: match record.terminal {
                                StreamConsumerTerminalV1::End(result) => {
                                    CommittedProducerStreamEventPayloadV1::End(result)
                                }
                                StreamConsumerTerminalV1::Cancel {
                                    role,
                                    reason,
                                    details,
                                } => CommittedProducerStreamEventPayloadV1::Cancel {
                                    role,
                                    reason,
                                    details,
                                },
                            },
                        },
                    ));
                }
                StreamSessionRecordV1::SourceUnavailable(record)
                    if record.key.session_key == self.session_key
                        && record.key.stream_id == stream_id =>
                {
                    events.push((
                        record.consumer_read_ordinal,
                        CommittedProducerStreamEventV1 {
                            stream_id,
                            producer_sequence: record.consumer_read_ordinal,
                            offset: record.source_offset,
                            terminal_author: None,
                            nested_handles: Vec::new(),
                            payload: CommittedProducerStreamEventPayloadV1::Cancel {
                                role: StreamCancelRoleV1::System,
                                reason: StreamCancelReasonV1::SourceUnavailable,
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

fn same_attachment_slot(left: &StreamAttachmentKeyV1, right: &StreamAttachmentKeyV1) -> bool {
    left.attachment_id == right.attachment_id && left.stream_id == right.stream_id
}

fn attachment_mismatch_status(
    persisted: &StreamAttachmentKeyV1,
    supplied: &StreamAttachmentKeyV1,
) -> ConsumerAttachmentStatus {
    let mut supplied_at_persisted_epoch = supplied.clone();
    supplied_at_persisted_epoch.epoch = persisted.epoch;
    if persisted == &supplied_at_persisted_epoch {
        ConsumerAttachmentStatus::EpochMismatch
    } else {
        ConsumerAttachmentStatus::IncarnationMismatch
    }
}

#[async_trait::async_trait]
impl StreamAttachmentConsumerProbe for DurableSessionStreams {
    async fn status(
        &self,
        key: &StreamAttachmentKeyV1,
    ) -> Result<ConsumerAttachmentStatus, DurableStreamProducerError> {
        self.topology_state(key, None)
            .await
            .map_err(DurableStreamProducerError::Oplog)
    }
}

pub(crate) struct DurableInputEndpoint {
    reader: Option<DurableStreamReader>,
    journal: VecDeque<CommittedProducerStreamEventV1>,
    streams: DurableSessionStreams,
    transport_stream_id: u64,
    handle: DurableStreamHandleV1,
    consumer_read_ordinal: u64,
    role: SessionStreamRoleV1,
}

pub(crate) struct ForwardedDurableInput {
    pub(crate) handle: DurableStreamHandleV1,
}

enum DurableStreamReader {
    Owned {
        reader: DurableCatchUpReader,
        source: Arc<DurableStreamProducer>,
        handle: DurableStreamHandleV1,
    },
    Attached(AttachedDurableCatchUpReader),
}

impl DurableStreamReader {
    async fn journal_lag_events(
        &self,
        after: Option<golem_common::model::durable_stream::StreamOffsetV1>,
    ) -> Result<usize, DurableStreamProducerError> {
        match self {
            Self::Owned { source, handle, .. } => source
                .read_segment(handle, after, None)
                .await
                .map(|events| events.len()),
            Self::Attached(reader) => reader
                .source
                .read_attached_segment(
                    &reader.attachment,
                    &reader.handle,
                    Timestamp::now_utc().to_millis(),
                    after,
                    None,
                )
                .await
                .map(|events| events.len()),
        }
    }

    async fn next(
        &mut self,
    ) -> Result<Option<CommittedProducerStreamEventV1>, DurableStreamProducerError> {
        match self {
            Self::Owned { reader, .. } => reader.next().await,
            Self::Attached(reader) => reader.next().await,
        }
    }
}

async fn record_source_journal_lag(
    reader: Option<&DurableStreamReader>,
    after: Option<golem_common::model::durable_stream::StreamOffsetV1>,
) {
    let lag = match reader {
        Some(reader) => match reader.journal_lag_events(after).await {
            Ok(lag) => lag,
            Err(error) => {
                tracing::debug!(
                    error = %error,
                    "Failed to sample durable consumer journal lag"
                );
                return;
            }
        },
        None => 0,
    };
    crate::metrics::durable_stream::record_journal_lag(lag);
}

struct AttachedDurableCatchUpReader {
    source: Arc<dyn AttachedStreamSegmentSource>,
    attachment: StreamAttachmentKeyV1,
    handle: DurableStreamHandleV1,
    consumer_journal: Option<Arc<dyn DurableStreamConsumerJournal>>,
    after: Option<golem_common::model::durable_stream::StreamOffsetV1>,
    buffered: VecDeque<CommittedProducerStreamEventV1>,
    terminal: bool,
}

impl AttachedDurableCatchUpReader {
    async fn source_unavailable_overlay(
        &self,
    ) -> Result<Option<CommittedProducerStreamEventV1>, DurableStreamProducerError> {
        let Some(journal) = &self.consumer_journal else {
            return Ok(None);
        };
        let source_offset = journal
            .source_unavailable(&self.attachment)
            .await
            .map_err(DurableStreamProducerError::Oplog)?;
        Ok(source_offset.map(|offset| CommittedProducerStreamEventV1 {
            stream_id: self.handle.stream_id,
            producer_sequence: 0,
            offset,
            terminal_author: None,
            nested_handles: Vec::new(),
            payload: CommittedProducerStreamEventPayloadV1::Cancel {
                role: StreamCancelRoleV1::System,
                reason: StreamCancelReasonV1::SourceUnavailable,
                details: None,
            },
        }))
    }

    async fn next(
        &mut self,
    ) -> Result<Option<CommittedProducerStreamEventV1>, DurableStreamProducerError> {
        loop {
            if let Some(event) = self.buffered.pop_front() {
                self.after = Some(event.offset);
                self.terminal = matches!(
                    event.payload,
                    CommittedProducerStreamEventPayloadV1::End(_)
                        | CommittedProducerStreamEventPayloadV1::Cancel { .. }
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
                .read_attached_segment(
                    &self.attachment,
                    &self.handle,
                    Timestamp::now_utc().to_millis(),
                    self.after,
                    None,
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
            if events.is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            } else {
                self.buffered.extend(events);
            }
        }
    }
}

type DurableReceiveFuture = Pin<
    Box<
        dyn Future<
                Output = Result<
                    (
                        Option<DurableStreamReader>,
                        Option<CommittedProducerStreamEventV1>,
                        HashMap<u64, DurableInputEndpoint>,
                        bool,
                    ),
                    String,
                >,
            > + Send
            + 'static,
    >,
>;

pub(crate) struct DurableInputProducer {
    reader: Option<DurableStreamReader>,
    journal: VecDeque<CommittedProducerStreamEventV1>,
    pending: Option<DurableReceiveFuture>,
    streams: DurableSessionStreams,
    transport_stream_id: u64,
    handle: DurableStreamHandleV1,
    consumer_read_ordinal: u64,
    role: SessionStreamRoleV1,
    finished: bool,
    dropping: bool,
}

impl DurableInputProducer {
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
        }
    }

    fn begin_receive(&mut self) {
        let mut reader = self.reader.take();
        let journaled_event = self.journal.pop_front();
        let streams = self.streams.clone();
        let stream_id = self.handle.stream_id;
        let ordinal = self.consumer_read_ordinal;
        let role = self.role;
        self.pending = Some(Box::pin(async move {
            let mut journaled = journaled_event.is_some();
            let event = match journaled_event {
                Some(event) => Some(event),
                None => reader
                    .as_mut()
                    .expect("durable input reader is missing")
                    .next()
                    .await
                    .map_err(|error| error.to_string())?,
            };
            if event.as_ref().is_some_and(|event| {
                matches!(
                    &event.payload,
                    CommittedProducerStreamEventPayloadV1::Cancel {
                        role: StreamCancelRoleV1::System,
                        reason: StreamCancelReasonV1::SourceUnavailable,
                        ..
                    }
                )
            }) {
                journaled = true;
            }
            let mut endpoints = HashMap::new();
            if let Some(event) = &event {
                let record = match &event.payload {
                    CommittedProducerStreamEventPayloadV1::Value(bytes) => {
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
                            let mapping = streams.ensure_nested_mapping(handle, role).await?;
                            endpoints.insert(
                                handle_index,
                                streams.endpoint(mapping.handle.clone(), 0, role).await?,
                            );
                            recursive_mappings.push(mapping);
                        }
                        StreamSessionRecordV1::ConsumerItemValue(StreamConsumerItemValueRecordV1 {
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
                    CommittedProducerStreamEventPayloadV1::PackedU8(byte) => {
                        StreamSessionRecordV1::ConsumerItemValue(StreamConsumerItemValueRecordV1 {
                            format_version: 1,
                            session_key: streams.session_key.clone(),
                            stream_id,
                            source_offset: event.offset,
                            consumer_read_ordinal: ordinal,
                            value: vec![*byte],
                            packed_u8: true,
                            recursive_handles: Vec::new(),
                            recursive_mappings: Vec::new(),
                        })
                    }
                    CommittedProducerStreamEventPayloadV1::End(result) => {
                        StreamSessionRecordV1::ConsumerTerminal(StreamConsumerTerminalRecordV1 {
                            format_version: 1,
                            session_key: streams.session_key.clone(),
                            stream_id,
                            source_offset: event.offset,
                            consumer_read_ordinal: ordinal,
                            terminal: StreamConsumerTerminalV1::End(result.clone()),
                        })
                    }
                    CommittedProducerStreamEventPayloadV1::Cancel {
                        role,
                        reason,
                        details,
                    } => StreamSessionRecordV1::ConsumerTerminal(StreamConsumerTerminalRecordV1 {
                        format_version: 1,
                        session_key: streams.session_key.clone(),
                        stream_id,
                        source_offset: event.offset,
                        consumer_read_ordinal: ordinal,
                        terminal: StreamConsumerTerminalV1::Cancel {
                            role: *role,
                            reason: *reason,
                            details: details.clone(),
                        },
                    }),
                };
                if !journaled {
                    streams.append_record(record).await;
                    streams.commit_consumer_journal().await?;
                    record_source_journal_lag(reader.as_ref(), Some(event.offset)).await;
                    tracing::debug!(
                        stream_id = %stream_id,
                        source_offset = %event.offset,
                        consumer_read_ordinal = ordinal,
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
            Ok((reader, event, endpoints, journaled))
        }));
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
                    SessionStreamRoleV1::Input => StreamCancelRoleV1::InputConsumer,
                    SessionStreamRoleV1::Output => StreamCancelRoleV1::OutputConsumer,
                };
                self.pending = Some(Box::pin(async move {
                    streams
                        .cancel_stream(
                            transport_stream_id,
                            role,
                            StreamCancelReasonV1::GuestDrop,
                            Some("guest dropped its durable readable stream end".to_string()),
                            None,
                        )
                        .await?;
                    Ok((None, None, HashMap::new(), false))
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
            self.begin_receive();
        }
        let (reader, event, mut endpoints, _journaled) =
            match self.pending.as_mut().unwrap().as_mut().poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Ok(result)) => result,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(wasmtime::Error::msg(error))),
            };
        self.pending = None;
        self.reader = reader;
        let Some(event) = event else {
            self.finished = true;
            return Poll::Ready(Err(wasmtime::Error::msg(
                "durable input stream source closed without a terminal event",
            )));
        };
        self.consumer_read_ordinal += 1;

        let value = match event.payload {
            CommittedProducerStreamEventPayloadV1::Value(bytes) => {
                let value = match ProtoSchemaValue::decode(bytes.as_slice()) {
                    Ok(value) => value,
                    Err(error) => {
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
                    Err(error) => return Poll::Ready(Err(wasmtime::Error::msg(error))),
                }
            }
            CommittedProducerStreamEventPayloadV1::PackedU8(byte) => SchemaValue::U8(byte),
            CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::Ok) => {
                self.finished = true;
                return Poll::Ready(Ok(StreamResult::Dropped));
            }
            CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::ErrorContext(error)) => {
                self.finished = true;
                return Poll::Ready(Err(wasmtime::Error::msg(format!(
                    "durable stream ended with error context: {error:?}"
                ))));
            }
            CommittedProducerStreamEventPayloadV1::Cancel {
                role,
                reason,
                details,
            } => {
                self.finished = true;
                return Poll::Ready(Err(wasmtime::Error::msg(format!(
                    "durable stream cancelled ({role:?}, {reason:?}): {}",
                    details.unwrap_or_default()
                ))));
            }
        };
        let encoded = {
            let mut resolver = StoreValueResolver::new(&mut store);
            match encode_value_with_streams(&value, &mut resolver) {
                Ok(encoded) => encoded,
                Err(error) => return Poll::Ready(Err(wasmtime::Error::msg(error.to_string()))),
            }
        };
        destination.set_buffer(Some(encoded));
        Poll::Ready(Ok(StreamResult::Completed))
    }

    fn try_into(me: Pin<Box<Self>>, ty: TypeId) -> Result<Box<dyn Any>, Pin<Box<Self>>> {
        let producer = me.as_ref().get_ref();
        if ty == TypeId::of::<ForwardedDurableInput>()
            && producer.consumer_read_ordinal == 0
            && producer.journal.is_empty()
            && producer.pending.is_none()
            && !producer.finished
        {
            Ok(Box::new(ForwardedDurableInput {
                handle: producer.handle.clone(),
            }))
        } else {
            Err(me)
        }
    }
}

fn stream_element_schema<'a>(
    graph: &'a SchemaGraph,
    root: &'a SchemaType,
    path: &[StreamValuePathStepV1],
) -> Result<Option<&'a SchemaType>, String> {
    let mut current = root;
    for step in path {
        current = graph
            .resolve_ref(current)
            .map_err(|error| error.to_string())?;
        current = match (step, current) {
            (StreamValuePathStepV1::RecordField(index), SchemaType::Record { fields, .. }) => {
                &fields
                    .get(*index as usize)
                    .ok_or_else(|| "stream record path is out of range".to_string())?
                    .body
            }
            (
                StreamValuePathStepV1::VariantCasePayload(index),
                SchemaType::Variant { cases, .. },
            ) => cases
                .get(*index as usize)
                .and_then(|case| case.payload.as_ref())
                .ok_or_else(|| "stream variant path has no payload".to_string())?,
            (StreamValuePathStepV1::TupleElement(index), SchemaType::Tuple { elements, .. }) => {
                elements
                    .get(*index as usize)
                    .ok_or_else(|| "stream tuple path is out of range".to_string())?
            }
            (StreamValuePathStepV1::ListElement(_), SchemaType::List { element, .. })
            | (StreamValuePathStepV1::FixedListElement(_), SchemaType::FixedList { element, .. }) => {
                element
            }
            (
                StreamValuePathStepV1::MapEntry {
                    side: golem_common::model::durable_stream::StreamMapSideV1::Key,
                    ..
                },
                SchemaType::Map { key, .. },
            ) => key,
            (
                StreamValuePathStepV1::MapEntry {
                    side: golem_common::model::durable_stream::StreamMapSideV1::Value,
                    ..
                },
                SchemaType::Map { value, .. },
            ) => value,
            (StreamValuePathStepV1::OptionSome, SchemaType::Option { inner, .. }) => inner,
            (StreamValuePathStepV1::ResultOk, SchemaType::Result { spec, .. }) => spec
                .ok
                .as_deref()
                .ok_or_else(|| "stream result ok path has no payload".to_string())?,
            (StreamValuePathStepV1::ResultErr, SchemaType::Result { spec, .. }) => spec
                .err
                .as_deref()
                .ok_or_else(|| "stream result error path has no payload".to_string())?,
            (StreamValuePathStepV1::UnionBranch(index), SchemaType::Union { spec, .. }) => {
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

fn collect_stream_paths(
    value: &ProtoSchemaValue,
    graph: &SchemaGraph,
    root: &SchemaType,
) -> Result<Vec<(u64, Vec<StreamValuePathStepV1>)>, String> {
    let mut result = Vec::new();
    decode_recursive_stream_value_with_schema(value.clone(), graph, root, |stream_id, path| {
        result.push((stream_id, path.to_vec()));
        Ok(SchemaValueStream::from_host_endpoint(()))
    })?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable_host::durable_stream::AttachedStreamSegmentSource;
    use crate::durable_host::durable_stream::tests::{
        TestIdentity, TestOplog, identity, registration,
    };
    use crate::durable_host::stream_transport::output_stream_pair;
    use crate::services::oplog::CommitLevel;
    use crate::services::rpc::{RpcDemand, RpcError};
    use golem_api_grpc::proto::golem::schema::{
        ListValue, SchemaValueStreamReference, schema_value,
    };
    use golem_common::base_model::component::{ComponentId, ComponentRevision};
    use golem_common::base_model::durable_stream::{
        AttachmentId, PersistedStreamInvocationDescriptorV1, ResumeAttemptDescriptorV1,
        StartAttemptDescriptorV1, StreamAttachmentKeyV1, StreamInvocationIdV1,
        StreamSessionAttachedRecordV1, StreamSessionMappingV1, StreamSessionPreparedRecordV1,
    };
    use golem_common::base_model::environment::EnvironmentId;
    use golem_common::base_model::{AgentFingerprint, AgentId, IdempotencyKey};
    use golem_common::model::account::AccountId;
    use golem_common::model::agent::InvocationFreshnessDisposition;
    use golem_common::model::invocation_context::TraceId;
    use golem_common::model::worker::AgentConfigEntryDto;
    use golem_common::model::{AgentInvocationPayload, OplogIndex, OwnedAgentId};
    use golem_schema::schema::schema_value::UnionValuePayload;
    use golem_schema::schema::{
        DiscriminatorRule, FieldDiscriminator, NamedFieldType, UnionBranch, UnionSpec,
    };
    use test_r::test;
    use uuid::Uuid;

    struct TestConsumerJournal(Arc<dyn Oplog>);

    #[async_trait::async_trait]
    impl DurableStreamConsumerJournal for TestConsumerJournal {
        async fn commit(&self) -> Result<(), String> {
            self.0.commit(CommitLevel::Always).await;
            Ok(())
        }
    }

    struct RecordingConsumerJournal {
        oplog: Arc<dyn Oplog>,
        commits: Arc<AtomicU64>,
    }

    struct AttachedProducerRpc {
        producer: Arc<DurableStreamProducer>,
    }

    #[async_trait::async_trait]
    impl Rpc for AttachedProducerRpc {
        async fn create_demand(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _self_created_by: AccountId,
            _self_agent_id: &AgentId,
            _self_env: &[(String, String)],
            _self_stack: golem_common::model::invocation_context::InvocationContextStack,
            _config: Vec<AgentConfigEntryDto>,
            _auth_ctx: &AuthCtx,
        ) -> Result<Box<dyn RpcDemand>, RpcError> {
            unreachable!("test RPC only serves attached stream segments")
        }

        async fn invoke_and_await(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _idempotency_key: Option<IdempotencyKey>,
            _freshness_disposition: InvocationFreshnessDisposition,
            _method_name: String,
            _method_parameters: SchemaValue,
            _self_created_by: AccountId,
            _self_agent_id: &AgentId,
            _self_env: &[(String, String)],
            _self_stack: golem_common::model::invocation_context::InvocationContextStack,
            _config: Vec<AgentConfigEntryDto>,
            _auth_ctx: &AuthCtx,
        ) -> Result<SchemaValue, RpcError> {
            unreachable!("test RPC only serves attached stream segments")
        }

        async fn read_durable_stream_segment(
            &self,
            request: golem_common::base_model::durable_stream::AttachedStreamSegmentRequestV1,
            _auth_ctx: &AuthCtx,
        ) -> Result<Vec<u8>, RpcError> {
            let events = self
                .producer
                .read_attached_segment(
                    &request.attachment,
                    &request.mapping.handle,
                    113,
                    request.after,
                    request.through,
                )
                .await
                .map_err(|error| RpcError::ProtocolError {
                    details: error.to_string(),
                })?;
            golem_common::serialization::serialize(&events).map_err(|error| {
                RpcError::ProtocolError {
                    details: error.to_string(),
                }
            })
        }

        async fn invoke(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _idempotency_key: Option<IdempotencyKey>,
            _freshness_disposition: InvocationFreshnessDisposition,
            _method_name: String,
            _method_parameters: SchemaValue,
            _self_created_by: AccountId,
            _self_agent_id: &AgentId,
            _self_env: &[(String, String)],
            _self_stack: golem_common::model::invocation_context::InvocationContextStack,
            _config: Vec<AgentConfigEntryDto>,
            _auth_ctx: &AuthCtx,
        ) -> Result<(), RpcError> {
            unreachable!("test RPC only serves attached stream segments")
        }
    }

    fn union_with_stream_in_second_branch() -> SchemaType {
        let field = |name: &str, body| NamedFieldType {
            name: name.to_string(),
            body,
            metadata: Default::default(),
        };
        SchemaType::union(UnionSpec {
            branches: vec![
                UnionBranch {
                    tag: "plain".to_string(),
                    body: SchemaType::record(vec![field("kind", SchemaType::string())]),
                    discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                        field_name: "kind".to_string(),
                        literal: Some("plain".to_string()),
                    }),
                    metadata: Default::default(),
                },
                UnionBranch {
                    tag: "stream".to_string(),
                    body: SchemaType::record(vec![
                        field("kind", SchemaType::string()),
                        field("values", SchemaType::stream(Some(SchemaType::u32()))),
                    ]),
                    discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                        field_name: "kind".to_string(),
                        literal: Some("stream".to_string()),
                    }),
                    metadata: Default::default(),
                },
            ],
        })
    }

    fn stream_union_value(stream: SchemaValueStream) -> SchemaValue {
        SchemaValue::Union(UnionValuePayload {
            tag: "stream".to_string(),
            body: Box::new(SchemaValue::Record {
                fields: vec![
                    SchemaValue::String("stream".to_string()),
                    SchemaValue::Stream(stream),
                ],
            }),
        })
    }

    #[test]
    async fn root_union_stream_coordinates_use_the_selected_branch_and_survive_reload() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = DurableStreamProducer::load(
            oplog.clone(),
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let streams = DurableSessionStreams::new(
            producer.clone(),
            oplog.clone(),
            identity.invocation.clone(),
            [],
        )
        .with_consumer_journal(Arc::new(TestConsumerJournal(oplog.clone())));
        let root = union_with_stream_in_second_branch();
        let graph = SchemaGraph::anonymous(root.clone());
        let path = vec![
            StreamValuePathStepV1::UnionBranch(1),
            StreamValuePathStepV1::RecordField(1),
        ];

        let (input_consumer, input_stream) = output_stream_pair(4, Arc::new(|| false)).unwrap();
        streams
            .materialize_agent_input(
                &stream_union_value(input_stream),
                &graph,
                &root,
                ComponentRevision::INITIAL,
            )
            .await
            .unwrap();
        let input_coordinate = StreamRegistrationCoordinateV1::Root {
            invocation_id: identity.invocation.clone(),
            root_kind: StreamRootKindV1::MethodInput,
            recursive_value_path: path.clone(),
        };
        let input_handle = producer
            .handle_for_coordinate(&input_coordinate)
            .await
            .expect("the caller input stream must use union branch 1");
        drop(input_consumer);

        let (output_consumer, output_stream) = output_stream_pair(4, Arc::new(|| false)).unwrap();
        drop(output_consumer);
        streams
            .materialize_result(
                stream_union_value(output_stream),
                &graph,
                &root,
                ComponentRevision::INITIAL,
            )
            .await
            .unwrap();
        let result_coordinate = StreamRegistrationCoordinateV1::Root {
            invocation_id: identity.invocation.clone(),
            root_kind: StreamRootKindV1::MethodResult,
            recursive_value_path: path,
        };
        let result_handle = producer
            .handle_for_coordinate(&result_coordinate)
            .await
            .expect("the callee result stream must use union branch 1");

        let reloaded = DurableStreamProducer::load(
            oplog,
            identity.environment_id,
            identity.agent_id,
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            reloaded.handle_for_coordinate(&input_coordinate).await,
            Some(input_handle)
        );
        assert_eq!(
            reloaded.handle_for_coordinate(&result_coordinate).await,
            Some(result_handle)
        );
    }

    #[async_trait::async_trait]
    impl DurableStreamConsumerJournal for RecordingConsumerJournal {
        async fn commit(&self) -> Result<(), String> {
            self.oplog.commit(CommitLevel::Always).await;
            self.commits.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }

    #[test]
    async fn owned_tail_backlog_uses_source_history() {
        /*
        async fn owned_live_tail_journal_lag_counts_committed_source_events() {
            */
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = DurableStreamProducer::load(
            oplog,
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let handle = producer
            .register(registration(
                &identity,
                StreamRegistrationCoordinateV1::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodInput,
                    recursive_value_path: Vec::new(),
                },
                StreamSourceKindV1::AgentHostedInput,
            ))
            .await
            .unwrap()
            .value;
        let mut reader = DurableStreamReader::Owned {
            reader: producer.catch_up(handle.clone(), None).await.unwrap(),
            source: producer.clone(),
            handle: handle.clone(),
        };

        producer
            .write_items(
                handle.stream_id,
                0,
                StreamItemsPayloadV1::PackedU8(vec![1, 2, 3]),
            )
            .await
            .unwrap();

        assert_eq!(reader.journal_lag_events(None).await.unwrap(), 3);
        let first = reader.next().await.unwrap().unwrap();
        assert_eq!(
            reader.journal_lag_events(Some(first.offset)).await.unwrap(),
            2
        );
    }

    #[test]
    async fn attached_preexisting_journal_lag_counts_committed_source_events() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = DurableStreamProducer::load(
            oplog,
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let handle = producer
            .register(registration(
                &identity,
                StreamRegistrationCoordinateV1::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodResult,
                    recursive_value_path: Vec::new(),
                },
                StreamSourceKindV1::InvocationOutput,
            ))
            .await
            .unwrap()
            .value;
        producer
            .write_items(
                handle.stream_id,
                0,
                StreamItemsPayloadV1::PackedU8(vec![1, 2, 3]),
            )
            .await
            .unwrap();
        let consumer_environment_id = EnvironmentId(Uuid::from_u128(41));
        let consumer = AgentId {
            component_id: ComponentId(Uuid::from_u128(42)),
            agent_id: "journal-lag-consumer".to_string(),
        };
        let consumer_fingerprint = AgentFingerprint(Uuid::from_u128(43));
        let consumer_invocation = StreamInvocationIdV1 {
            callee_environment_id: consumer_environment_id,
            callee: consumer.clone(),
            callee_fingerprint: consumer_fingerprint,
            idempotency_key: IdempotencyKey::new("journal-lag-consumer-invocation".to_string()),
        };
        let attachment = StreamAttachmentKeyV1 {
            attachment_id: AttachmentId::primary(
                consumer_environment_id,
                &consumer,
                &consumer_invocation.idempotency_key,
            )
            .unwrap(),
            stream_id: handle.stream_id,
            epoch: 1,
            session_key: consumer_invocation.clone(),
            producer_environment_id: identity.environment_id,
            producer: identity.agent_id,
            expected_producer_fingerprint: identity.fingerprint,
            consumer_environment_id,
            consumer,
            expected_consumer_fingerprint: consumer_fingerprint,
            consumer_invocation,
        };
        let now_millis = Timestamp::now_utc().to_millis();
        producer
            .prepare_attachment(attachment.clone(), now_millis)
            .await
            .unwrap();
        producer
            .activate_attachment(attachment.clone(), now_millis)
            .await
            .unwrap();
        let mut reader = DurableStreamReader::Attached(AttachedDurableCatchUpReader {
            source: producer,
            attachment,
            handle,
            consumer_journal: None,
            after: None,
            buffered: VecDeque::new(),
            terminal: false,
        });

        assert_eq!(reader.journal_lag_events(None).await.unwrap(), 3);
        let first = reader.next().await.unwrap().unwrap();
        assert_eq!(
            reader.journal_lag_events(Some(first.offset)).await.unwrap(),
            2
        );
    }

    #[test]
    async fn consumer_value_is_committed_before_delivery_and_replay_is_a_no_op() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = DurableStreamProducer::load(
            oplog.clone(),
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let handle = producer
            .register(registration(
                &identity,
                StreamRegistrationCoordinateV1::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodInput,
                    recursive_value_path: Vec::new(),
                },
                StreamSourceKindV1::AgentHostedInput,
            ))
            .await
            .unwrap()
            .value;
        producer
            .write_items(
                handle.stream_id,
                0,
                StreamItemsPayloadV1::Values(vec![
                    ProtoSchemaValue::try_from(SchemaValue::U32(42))
                        .unwrap()
                        .encode_to_vec(),
                ]),
            )
            .await
            .unwrap();

        let commits = Arc::new(AtomicU64::new(0));
        let streams = DurableSessionStreams::new(
            producer,
            oplog.clone(),
            identity.invocation,
            [(1, handle.clone(), SessionStreamRoleV1::Input)],
        )
        .with_consumer_journal(Arc::new(RecordingConsumerJournal {
            oplog,
            commits: commits.clone(),
        }));
        let mut first = DurableInputProducer::new(
            streams
                .endpoint(handle.clone(), 0, SessionStreamRoleV1::Input)
                .await
                .unwrap(),
        );
        first.begin_receive();
        let (_, event, _, journaled) = first.pending.take().unwrap().await.unwrap();
        assert!(!journaled);
        assert!(event.is_some());
        assert_eq!(commits.load(Ordering::Relaxed), 1);

        let mut replay = DurableInputProducer::new(
            streams
                .endpoint(handle, 0, SessionStreamRoleV1::Input)
                .await
                .unwrap(),
        );
        replay.begin_receive();
        let (_, event, _, journaled) = replay.pending.take().unwrap().await.unwrap();
        assert!(journaled);
        assert!(event.is_some());
        assert_eq!(commits.load(Ordering::Relaxed), 1);
    }

    #[test]
    async fn source_unavailable_overlay_replays_without_reopening_the_source() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = DurableStreamProducer::load(
            oplog.clone(),
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let handle = producer
            .register(registration(
                &identity,
                StreamRegistrationCoordinateV1::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodInput,
                    recursive_value_path: Vec::new(),
                },
                StreamSourceKindV1::AgentHostedInput,
            ))
            .await
            .unwrap()
            .value;
        let streams = DurableSessionStreams::new(
            producer,
            oplog,
            identity.invocation,
            [(1, handle.clone(), SessionStreamRoleV1::Input)],
        );
        streams
            .append_record(StreamSessionRecordV1::SourceUnavailable(
                golem_common::base_model::durable_stream::StreamSourceUnavailableRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    key: streams.attachment_key(&handle, 1).unwrap(),
                    source_offset: golem_common::model::durable_stream::StreamOffsetV1::new(
                        OplogIndex::INITIAL,
                        0,
                    ),
                    consumer_read_ordinal: 0,
                },
            ))
            .await;

        let endpoint = streams
            .endpoint(handle, 0, SessionStreamRoleV1::Input)
            .await
            .unwrap();
        assert!(endpoint.reader.is_none());
        let mut replay = DurableInputProducer::new(endpoint);
        replay.begin_receive();
        let (_, event, _, journaled) = replay.pending.take().unwrap().await.unwrap();
        assert!(journaled);
        assert!(matches!(
            event.unwrap().payload,
            CommittedProducerStreamEventPayloadV1::Cancel {
                role: StreamCancelRoleV1::System,
                reason: StreamCancelReasonV1::SourceUnavailable,
                details: None,
            }
        ));
    }

    #[test]
    async fn detach_resume_and_takeover_advance_authority_and_fence_old_epochs() {
        let identity = identity();
        let attachment_id = AttachmentId::primary(
            identity.environment_id,
            &identity.agent_id,
            &identity.invocation.idempotency_key,
        )
        .unwrap();
        let oplog = Arc::new(TestOplog::default());
        let producer = DurableStreamProducer::load(
            oplog.clone(),
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let handle = producer
            .register(registration(
                &identity,
                StreamRegistrationCoordinateV1::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodInput,
                    recursive_value_path: Vec::new(),
                },
                StreamSourceKindV1::AgentHostedInput,
            ))
            .await
            .unwrap()
            .value;
        let mapping = StreamSessionMappingRecordV1 {
            transport_stream_id: 7,
            handle: handle.clone(),
            role: SessionStreamRoleV1::Input,
        };
        let output_handle = producer
            .register(registration(
                &identity,
                StreamRegistrationCoordinateV1::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodResult,
                    recursive_value_path: Vec::new(),
                },
                StreamSourceKindV1::InvocationOutput,
            ))
            .await
            .unwrap()
            .value;
        let start_attempt_id = AttemptId::fresh();
        producer
            .append_session_record(StreamSessionRecordV1::Prepared(
                StreamSessionPreparedRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    attempt: StartAttemptDescriptorV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: identity.invocation.clone(),
                        attachment_id,
                        expected_callee_fingerprint: identity.fingerprint,
                        attempt_id: start_attempt_id,
                        invocation: PersistedStreamInvocationDescriptorV1 {
                            format_version: DURABLE_STREAM_FORMAT_VERSION,
                            session_key: identity.invocation.clone(),
                            target_component_revision: ComponentRevision::INITIAL,
                            method_name: "consume".to_string(),
                            invocation_value: vec![1],
                            stream_handles: vec![handle.clone()],
                            execution_config: vec![2],
                            effective_identity: vec![3],
                        },
                        effective_identity: vec![3],
                        live_join_buffer_events: 8,
                    },
                    stream_mappings: vec![mapping.clone()],
                },
            ))
            .await
            .unwrap();
        let pending_invocation_oplog_index = oplog
            .add(OplogEntry::pending_agent_invocation(
                identity.invocation.idempotency_key.clone(),
                OplogPayload::Inline(Box::new(AgentInvocationPayload::SaveSnapshot)),
                TraceId::generate(),
                Vec::new(),
                Vec::new(),
            ))
            .await;
        producer
            .append_session_record(StreamSessionRecordV1::Attached(
                StreamSessionAttachedRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: identity.invocation.clone(),
                    attachment_id,
                    attempt_id: start_attempt_id,
                    epoch: 1,
                    pending_invocation_oplog_index,
                },
            ))
            .await
            .unwrap();
        let streams_for = |epoch, attempt_id| {
            DurableSessionStreams::new(
                producer.clone(),
                oplog.clone(),
                identity.invocation.clone(),
                [
                    (7, handle.clone(), SessionStreamRoleV1::Input),
                    (8, output_handle.clone(), SessionStreamRoleV1::Output),
                ],
            )
            .with_consumer_journal(Arc::new(TestConsumerJournal(oplog.clone())))
            .with_attachment(epoch, attempt_id)
        };
        let attempt = |operation, expected_epoch, attempt_id| ResumeAttemptDescriptorV1 {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            operation,
            session_key: identity.invocation.clone(),
            attachment_id,
            expected_callee_fingerprint: identity.fingerprint,
            attempt_id,
            expected_epoch,
            effective_identity: vec![3],
            cursors: Vec::new(),
            live_join_buffer_events: 8,
        };

        let epoch1 = streams_for(1, start_attempt_id);
        epoch1.ensure_current_attachment().await.unwrap();
        assert!(epoch1.detach_current().await.unwrap());
        assert!(epoch1.ensure_current_attachment().await.is_err());
        producer
            .write_items(
                output_handle.stream_id,
                0,
                StreamItemsPayloadV1::Values(vec![
                    ProtoSchemaValue::try_from(SchemaValue::U32(42))
                        .unwrap()
                        .encode_to_vec(),
                ]),
            )
            .await
            .unwrap();
        let after_detach = oplog.current_oplog_index().await;
        assert!(!epoch1.detach_current().await.unwrap());
        assert_eq!(oplog.current_oplog_index().await, after_detach);

        for (operation, expected_epoch, accepted_epoch, expected_error) in [
            (
                StreamResumeOperationV1::Takeover,
                1,
                2,
                "InvalidAttachmentState",
            ),
            (StreamResumeOperationV1::Resume, 0, 1, "StaleEpoch"),
            (StreamResumeOperationV1::Resume, 2, 3, "InvalidEpoch"),
        ] {
            let error = epoch1
                .commit_resume_attempt(StreamSessionResumeAttemptRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    attempt: attempt(operation, expected_epoch, AttemptId::fresh()),
                    accepted_epoch,
                })
                .await
                .unwrap_err();
            assert!(error.contains(expected_error), "unexpected error: {error}");
            assert_eq!(oplog.current_oplog_index().await, after_detach);
        }

        let resume_attempt_id = AttemptId::fresh();
        epoch1
            .commit_resume_attempt(StreamSessionResumeAttemptRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                attempt: attempt(StreamResumeOperationV1::Resume, 1, resume_attempt_id),
                accepted_epoch: 2,
            })
            .await
            .unwrap();
        let epoch2 = streams_for(2, resume_attempt_id);
        epoch2.ensure_current_attachment().await.unwrap();
        assert!(epoch1.ensure_current_attachment().await.is_err());
        assert!(
            epoch1
                .validate_frame(
                    7,
                    Some(handle.stream_id.0.into()),
                    1,
                    SessionStreamRoleV1::Input,
                )
                .await
                .unwrap_err()
                .contains("StaleEpoch")
        );
        let before_old_detach = oplog.current_oplog_index().await;
        assert!(!epoch1.detach_current().await.unwrap());
        assert_eq!(oplog.current_oplog_index().await, before_old_detach);

        let takeover_attempt_id = AttemptId::fresh();
        epoch2
            .commit_resume_attempt(StreamSessionResumeAttemptRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                attempt: attempt(StreamResumeOperationV1::Takeover, 2, takeover_attempt_id),
                accepted_epoch: 3,
            })
            .await
            .unwrap();
        let epoch3 = streams_for(3, takeover_attempt_id);
        epoch3.ensure_current_attachment().await.unwrap();
        assert!(epoch2.ensure_current_attachment().await.is_err());
        epoch3
            .validate_frame(
                7,
                Some(handle.stream_id.0.into()),
                3,
                SessionStreamRoleV1::Input,
            )
            .await
            .unwrap();

        epoch3
            .cancel_stream(
                7,
                StreamCancelRoleV1::InputProducer,
                StreamCancelReasonV1::Cancelled,
                Some("explicit input cancellation".to_string()),
                Some(3),
            )
            .await
            .unwrap();
        epoch3
            .cancel_stream(
                8,
                StreamCancelRoleV1::OutputConsumer,
                StreamCancelReasonV1::GuestDrop,
                Some("guest dropped output readable end".to_string()),
                Some(3),
            )
            .await
            .unwrap();

        let after_cancellations = oplog.current_oplog_index().await;
        epoch3
            .cancel_stream(
                7,
                StreamCancelRoleV1::InputProducer,
                StreamCancelReasonV1::Cancelled,
                Some("explicit input cancellation".to_string()),
                Some(3),
            )
            .await
            .unwrap();
        epoch3
            .cancel_stream(
                8,
                StreamCancelRoleV1::OutputConsumer,
                StreamCancelReasonV1::GuestDrop,
                Some("guest dropped output readable end".to_string()),
                Some(3),
            )
            .await
            .unwrap();
        assert_eq!(oplog.current_oplog_index().await, after_cancellations);

        let current = oplog.current_oplog_index().await;
        let mut intent_indexes = HashMap::new();
        let mut terminal_indexes = HashMap::new();
        for (index, entry) in oplog.read_many(OplogIndex::INITIAL, current.as_u64()).await {
            match entry {
                OplogEntry::StreamSession { record, .. } => {
                    if let StreamSessionRecordV1::ConsumerCancelIntent(record) =
                        epoch3.download_record(record).await.unwrap()
                    {
                        intent_indexes
                            .insert(record.stream_id, (index, record.role, record.reason));
                    }
                }
                OplogEntry::StreamCancel { record, .. } => {
                    let record = oplog.download_payload(record).await.unwrap();
                    terminal_indexes.insert(record.stream_id, (index, record.role, record.reason));
                }
                _ => {}
            }
        }
        for (stream_id, expected_role, expected_reason) in [
            (
                handle.stream_id,
                StreamCancelRoleV1::InputProducer,
                StreamCancelReasonV1::Cancelled,
            ),
            (
                output_handle.stream_id,
                StreamCancelRoleV1::OutputConsumer,
                StreamCancelReasonV1::GuestDrop,
            ),
        ] {
            let (intent_index, intent_role, intent_reason) = intent_indexes[&stream_id];
            let (terminal_index, terminal_role, terminal_reason) = terminal_indexes[&stream_id];
            assert!(intent_index < terminal_index);
            assert_eq!(intent_role, expected_role);
            assert_eq!(terminal_role, expected_role);
            assert_eq!(intent_reason, expected_reason);
            assert_eq!(terminal_reason, expected_reason);
        }
    }

    #[test]
    async fn nested_consumer_mappings_preserve_the_parent_input_or_output_role() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = DurableStreamProducer::load(
            oplog.clone(),
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let mut roots = Vec::new();
        let mut nested_handles = Vec::new();
        for (root_kind, role, attachment_id) in [
            (
                StreamRootKindV1::MethodInput,
                SessionStreamRoleV1::Input,
                101,
            ),
            (
                StreamRootKindV1::MethodResult,
                SessionStreamRoleV1::Output,
                102,
            ),
        ] {
            let mut root_request = registration(
                &identity,
                StreamRegistrationCoordinateV1::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind,
                    recursive_value_path: Vec::new(),
                },
                match role {
                    SessionStreamRoleV1::Input => StreamSourceKindV1::AgentHostedInput,
                    SessionStreamRoleV1::Output => StreamSourceKindV1::InvocationOutput,
                },
            );
            root_request.session_mapping = Some(StreamSessionMappingV1 {
                session_key: identity.invocation.clone(),
                attachment_id: AttachmentId(Uuid::from_u128(attachment_id)),
                role,
            });
            let root = producer.register(root_request).await.unwrap().value;
            let nested_request = registration(
                &identity,
                StreamRegistrationCoordinateV1::Nested {
                    parent_stream_id: root.stream_id,
                    parent_producer_sequence: 0,
                    recursive_value_path: Vec::new(),
                },
                StreamSourceKindV1::Nested,
            );
            producer
                .write_items_with_nested(
                    root.stream_id,
                    0,
                    StreamItemsPayloadV1::Values(vec![
                        ProtoSchemaValue {
                            value: Some(schema_value::Value::StreamReference(
                                SchemaValueStreamReference { stream_id: 0 },
                            )),
                        }
                        .encode_to_vec(),
                    ]),
                    vec![nested_request],
                )
                .await
                .unwrap();
            let nested = producer.nested_handles(root.stream_id, 0).await.unwrap()[0].clone();
            roots.push((role, root));
            nested_handles.push((role, nested));
        }

        let streams = DurableSessionStreams::new(
            producer.clone(),
            oplog.clone(),
            identity.invocation.clone(),
            roots
                .iter()
                .enumerate()
                .map(|(index, (role, handle))| (index as u64, handle.clone(), *role)),
        )
        .with_consumer_journal(Arc::new(TestConsumerJournal(oplog.clone())));
        for (role, root) in &roots {
            let mut consumer =
                DurableInputProducer::new(streams.endpoint(root.clone(), 0, *role).await.unwrap());
            consumer.begin_receive();
            let (_, event, nested, journaled) = consumer.pending.take().unwrap().await.unwrap();
            assert!(!journaled);
            assert!(event.is_some());
            assert_eq!(nested.len(), 1);
        }

        let mut persisted_roles = HashMap::new();
        let current = oplog.current_oplog_index().await;
        for (_, entry) in oplog.read_many(OplogIndex::INITIAL, current.as_u64()).await {
            let OplogEntry::StreamSession { record, .. } = entry else {
                continue;
            };
            if let StreamSessionRecordV1::ConsumerItemValue(record) =
                streams.download_record(record).await.unwrap()
                && let [mapping] = record.recursive_mappings.as_slice()
            {
                persisted_roles.insert(record.stream_id, mapping.role);
            }
        }
        for ((role, root), (_, nested)) in roots.iter().zip(&nested_handles) {
            assert_eq!(persisted_roles.get(&root.stream_id), Some(role));
            assert!(streams.mapping_for_handle(nested, *role).is_some());
        }

        let restarted = DurableSessionStreams::new(
            producer,
            oplog,
            identity.invocation,
            roots
                .iter()
                .enumerate()
                .map(|(index, (role, handle))| (index as u64, handle.clone(), *role)),
        );
        restarted.recover_session_mappings().await.unwrap();
        for (role, nested) in nested_handles {
            assert!(restarted.mapping_for_handle(&nested, role).is_some());
            assert!(
                restarted
                    .mapping_for_handle(
                        &nested,
                        match role {
                            SessionStreamRoleV1::Input => SessionStreamRoleV1::Output,
                            SessionStreamRoleV1::Output => SessionStreamRoleV1::Input,
                        },
                    )
                    .is_none()
            );
        }
    }

    #[test]
    async fn forwarded_topology_is_committed_before_visibility_and_replays_exactly() {
        let consumer = identity();
        let producer_identity = TestIdentity {
            environment_id: EnvironmentId(Uuid::from_u128(21)),
            agent_id: AgentId {
                component_id: ComponentId(Uuid::from_u128(22)),
                agent_id: "remote-producer".to_string(),
            },
            fingerprint: AgentFingerprint(Uuid::from_u128(23)),
            invocation: StreamInvocationIdV1 {
                callee_environment_id: EnvironmentId(Uuid::from_u128(21)),
                callee: AgentId {
                    component_id: ComponentId(Uuid::from_u128(22)),
                    agent_id: "remote-producer".to_string(),
                },
                callee_fingerprint: AgentFingerprint(Uuid::from_u128(23)),
                idempotency_key: IdempotencyKey::new("remote-invocation".to_string()),
            },
        };
        let producer_oplog = Arc::new(TestOplog::default());
        let remote_producer = DurableStreamProducer::load(
            producer_oplog.clone(),
            producer_identity.environment_id,
            producer_identity.agent_id.clone(),
            producer_identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let handle = remote_producer
            .register(registration(
                &producer_identity,
                StreamRegistrationCoordinateV1::Root {
                    invocation_id: producer_identity.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodResult,
                    recursive_value_path: Vec::new(),
                },
                StreamSourceKindV1::InvocationOutput,
            ))
            .await
            .unwrap()
            .value;
        let consumer_oplog = Arc::new(TestOplog::default());
        let consumer_producer = DurableStreamProducer::load(
            consumer_oplog.clone(),
            consumer.environment_id,
            consumer.agent_id.clone(),
            consumer.fingerprint,
            None,
        )
        .await
        .unwrap();
        let streams = DurableSessionStreams::new(
            consumer_producer.clone(),
            consumer_oplog.clone(),
            consumer.invocation.clone(),
            [],
        )
        .with_consumer_journal(Arc::new(TestConsumerJournal(consumer_oplog.clone())))
        .with_rpc(Arc::new(AttachedProducerRpc {
            producer: remote_producer.clone(),
        }))
        .with_auth_ctx(AuthCtx::System);
        let attachment = StreamAttachmentKeyV1 {
            attachment_id: AttachmentId::primary(
                consumer.environment_id,
                &consumer.agent_id,
                &consumer.invocation.idempotency_key,
            )
            .unwrap(),
            stream_id: handle.stream_id,
            epoch: 1,
            session_key: consumer.invocation.clone(),
            producer_environment_id: producer_identity.environment_id,
            producer: producer_identity.agent_id.clone(),
            expected_producer_fingerprint: producer_identity.fingerprint,
            consumer_environment_id: consumer.environment_id,
            consumer: consumer.agent_id.clone(),
            expected_consumer_fingerprint: consumer.fingerprint,
            consumer_invocation: consumer.invocation.clone(),
        };
        let mapping = StreamSessionMappingRecordV1 {
            transport_stream_id: 17,
            handle: handle.clone(),
            role: SessionStreamRoleV1::Input,
        };
        let attempt_id = AttemptId::fresh();
        streams
            .append_record(StreamSessionRecordV1::Prepared(
                StreamSessionPreparedRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    attempt: StartAttemptDescriptorV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: consumer.invocation.clone(),
                        attachment_id: attachment.attachment_id,
                        expected_callee_fingerprint: consumer.fingerprint,
                        attempt_id,
                        invocation: PersistedStreamInvocationDescriptorV1 {
                            format_version: DURABLE_STREAM_FORMAT_VERSION,
                            session_key: consumer.invocation.clone(),
                            target_component_revision: ComponentRevision::INITIAL,
                            method_name: "forward".to_string(),
                            invocation_value: vec![1],
                            stream_handles: vec![handle.clone()],
                            execution_config: vec![2],
                            effective_identity: vec![3],
                        },
                        effective_identity: vec![3],
                        live_join_buffer_events: 8,
                    },
                    stream_mappings: vec![mapping.clone()],
                },
            ))
            .await;
        let pending_invocation_oplog_index = consumer_oplog
            .add(OplogEntry::pending_agent_invocation(
                consumer.invocation.idempotency_key.clone(),
                OplogPayload::Inline(Box::new(AgentInvocationPayload::SaveSnapshot)),
                TraceId::generate(),
                Vec::new(),
                Vec::new(),
            ))
            .await;
        streams
            .append_record(StreamSessionRecordV1::Attached(
                golem_common::base_model::durable_stream::StreamSessionAttachedRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: consumer.invocation.clone(),
                    attachment_id: attachment.attachment_id,
                    attempt_id,
                    epoch: 1,
                    pending_invocation_oplog_index,
                },
            ))
            .await;
        let streams = streams.with_attachment(1, attempt_id);
        remote_producer
            .prepare_attachment(attachment.clone(), 100)
            .await
            .unwrap();
        assert!(streams.handle(17).is_none());
        streams
            .activate_forwarded_mapping(
                attachment.clone(),
                mapping.clone(),
                remote_producer.as_ref(),
                110,
            )
            .await
            .unwrap();
        assert_eq!(streams.handle(17), Some(handle.clone()));
        assert_eq!(
            StreamAttachmentConsumerProbe::status(&streams, &attachment)
                .await
                .unwrap(),
            ConsumerAttachmentStatus::Active
        );
        let output_mapping = StreamSessionMappingRecordV1 {
            transport_stream_id: 18,
            handle: handle.clone(),
            role: SessionStreamRoleV1::Output,
        };
        streams
            .activate_forwarded_mapping(
                attachment.clone(),
                output_mapping.clone(),
                remote_producer.as_ref(),
                112,
            )
            .await
            .unwrap();
        assert_eq!(streams.handle(18), Some(handle.clone()));
        assert!(
            streams
                .validate_frame(
                    18,
                    Some(handle.stream_id.0.into()),
                    1,
                    SessionStreamRoleV1::Input,
                )
                .await
                .is_err()
        );
        assert!(
            streams
                .validate_frame(
                    17,
                    Some(handle.stream_id.0.into()),
                    1,
                    SessionStreamRoleV1::Output,
                )
                .await
                .is_err()
        );
        assert_eq!(
            StreamAttachmentConsumerProbe::status_exact(
                &streams,
                &attachment,
                Some(&output_mapping),
            )
            .await
            .unwrap(),
            ConsumerAttachmentStatus::Active
        );
        assert!(streams.input_high_waters().await.unwrap().is_empty());
        assert!(
            remote_producer
                .read_attached_segment(&attachment, &handle, 111, None, None)
                .await
                .unwrap()
                .is_empty()
        );
        let consumer_length = consumer_oplog.current_oplog_index().await;
        let producer_length = producer_oplog.current_oplog_index().await;
        streams
            .activate_forwarded_mapping(
                attachment.clone(),
                mapping.clone(),
                remote_producer.as_ref(),
                120,
            )
            .await
            .unwrap();
        assert_eq!(consumer_oplog.current_oplog_index().await, consumer_length);
        assert_eq!(producer_oplog.current_oplog_index().await, producer_length);

        remote_producer
            .write_items(
                handle.stream_id,
                0,
                StreamItemsPayloadV1::Values(vec![
                    ProtoSchemaValue::try_from(SchemaValue::U32(42))
                        .unwrap()
                        .encode_to_vec(),
                ]),
            )
            .await
            .unwrap();
        remote_producer
            .end(handle.stream_id, 1, StreamEndResultV1::Ok)
            .await
            .unwrap();
        let (responses, mut response_stream) = mpsc::channel(2);
        assert!(
            streams
                .pump_output_stream_from(18, handle.clone(), None, &responses)
                .await
                .unwrap()
                .is_empty()
        );
        let Some(invocation_response::Response::OutputItem(item)) =
            response_stream.recv().await.unwrap().response
        else {
            panic!("forwarded output must be read through its attached producer")
        };
        assert_eq!(item.transport_stream_id, 18);
        assert_eq!(item.producer_sequence, 0);
        let Some(invocation_response::Response::OutputEnd(end)) =
            response_stream.recv().await.unwrap().response
        else {
            panic!("forwarded output terminal must be read through its attached producer")
        };
        assert_eq!(end.transport_stream_id, 18);
        assert_eq!(end.producer_sequence, 1);
        let producer_length_after_output = producer_oplog.current_oplog_index().await;

        let conflicting_mapping = StreamSessionMappingRecordV1 {
            transport_stream_id: 18,
            ..mapping
        };
        assert!(
            streams
                .activate_forwarded_mapping(
                    attachment.clone(),
                    conflicting_mapping,
                    remote_producer.as_ref(),
                    121,
                )
                .await
                .is_err()
        );
        assert_eq!(
            producer_oplog.current_oplog_index().await,
            producer_length_after_output
        );

        let restarted = DurableSessionStreams::new(
            consumer_producer,
            consumer_oplog.clone(),
            consumer.invocation,
            [],
        )
        .with_consumer_journal(Arc::new(TestConsumerJournal(consumer_oplog)));
        restarted.recover_session_mappings().await.unwrap();
        assert_eq!(restarted.handle(17), Some(handle));

        let mut future_epoch = attachment.clone();
        future_epoch.epoch = 2;
        assert_eq!(
            StreamAttachmentConsumerProbe::status(&restarted, &future_epoch)
                .await
                .unwrap(),
            ConsumerAttachmentStatus::EpochMismatch
        );

        let mut recreated = attachment.clone();
        recreated.expected_consumer_fingerprint = AgentFingerprint(Uuid::from_u128(24));
        assert_eq!(
            StreamAttachmentConsumerProbe::status(&restarted, &recreated)
                .await
                .unwrap(),
            ConsumerAttachmentStatus::IncarnationMismatch
        );

        let second_handle = remote_producer
            .register(ProducerRegistrationRequestV1 {
                coordinate: StreamRegistrationCoordinateV1::Root {
                    invocation_id: producer_identity.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodResult,
                    recursive_value_path: vec![StreamValuePathStepV1::ListElement(1)],
                },
                ..registration(
                    &producer_identity,
                    StreamRegistrationCoordinateV1::Root {
                        invocation_id: producer_identity.invocation.clone(),
                        root_kind: StreamRootKindV1::MethodResult,
                        recursive_value_path: Vec::new(),
                    },
                    StreamSourceKindV1::InvocationOutput,
                )
            })
            .await
            .unwrap()
            .value;
        let partial_attachment = StreamAttachmentKeyV1 {
            stream_id: second_handle.stream_id,
            epoch: 2,
            ..attachment.clone()
        };
        let partial_mapping = StreamSessionMappingRecordV1 {
            transport_stream_id: 19,
            handle: second_handle,
            role: SessionStreamRoleV1::Input,
        };
        restarted
            .append_record(StreamSessionRecordV1::TopologyPrepared(
                StreamTopologyPreparedRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: restarted.session_key.clone(),
                    attachment: partial_attachment.clone(),
                    mapping: partial_mapping.clone(),
                },
            ))
            .await;
        let consumer_length = restarted.oplog.current_oplog_index().await;
        let producer_length = producer_oplog.current_oplog_index().await;
        assert!(
            restarted
                .activate_forwarded_mapping(
                    partial_attachment.clone(),
                    partial_mapping.clone(),
                    remote_producer.as_ref(),
                    130,
                )
                .await
                .is_err()
        );
        assert_eq!(restarted.oplog.current_oplog_index().await, consumer_length);
        assert_eq!(producer_oplog.current_oplog_index().await, producer_length);
        assert!(restarted.handle(19).is_none());
        assert!(restarted.complete().await.is_err());
        restarted
            .append_record(StreamSessionRecordV1::TopologyActivated(
                StreamTopologyActivatedRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: restarted.session_key.clone(),
                    attachment: partial_attachment,
                    mapping: partial_mapping,
                },
            ))
            .await;
        assert!(restarted.complete().await.is_err());
    }

    #[test]
    async fn local_topology_cannot_activate_before_exact_session_attachment() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = DurableStreamProducer::load(
            oplog.clone(),
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let handle = producer
            .register(registration(
                &identity,
                StreamRegistrationCoordinateV1::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodInput,
                    recursive_value_path: Vec::new(),
                },
                StreamSourceKindV1::AgentHostedInput,
            ))
            .await
            .unwrap()
            .value;
        let attachment_id = AttachmentId::primary(
            identity.environment_id,
            &identity.agent_id,
            &identity.invocation.idempotency_key,
        )
        .unwrap();
        let attempt_id = AttemptId::fresh();
        let mapping = StreamSessionMappingRecordV1 {
            transport_stream_id: 17,
            handle: handle.clone(),
            role: SessionStreamRoleV1::Input,
        };
        producer
            .append_session_record(StreamSessionRecordV1::Prepared(
                StreamSessionPreparedRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    attempt: StartAttemptDescriptorV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: identity.invocation.clone(),
                        attachment_id,
                        expected_callee_fingerprint: identity.fingerprint,
                        attempt_id,
                        invocation: PersistedStreamInvocationDescriptorV1 {
                            format_version: DURABLE_STREAM_FORMAT_VERSION,
                            session_key: identity.invocation.clone(),
                            target_component_revision: ComponentRevision::INITIAL,
                            method_name: "consume".to_string(),
                            invocation_value: vec![1],
                            stream_handles: vec![handle.clone()],
                            execution_config: vec![2],
                            effective_identity: vec![3],
                        },
                        effective_identity: vec![3],
                        live_join_buffer_events: 8,
                    },
                    stream_mappings: vec![mapping.clone()],
                },
            ))
            .await
            .unwrap();
        let attachment = StreamAttachmentKeyV1 {
            attachment_id,
            stream_id: handle.stream_id,
            epoch: 1,
            session_key: identity.invocation.clone(),
            producer_environment_id: identity.environment_id,
            producer: identity.agent_id.clone(),
            expected_producer_fingerprint: identity.fingerprint,
            consumer_environment_id: identity.environment_id,
            consumer: identity.agent_id.clone(),
            expected_consumer_fingerprint: identity.fingerprint,
            consumer_invocation: identity.invocation.clone(),
        };
        let streams = DurableSessionStreams::new(
            producer.clone(),
            oplog.clone(),
            identity.invocation.clone(),
            [],
        )
        .with_consumer_journal(Arc::new(TestConsumerJournal(oplog.clone())));

        assert!(
            streams
                .activate_forwarded_mapping(
                    attachment.clone(),
                    mapping.clone(),
                    producer.as_ref(),
                    100,
                )
                .await
                .is_err()
        );
        assert_eq!(
            streams
                .topology_state(&attachment, Some(&mapping))
                .await
                .unwrap(),
            ConsumerAttachmentStatus::Prepared
        );
        assert!(streams.handle(mapping.transport_stream_id).is_none());

        let pending_invocation_oplog_index = oplog
            .add(OplogEntry::pending_agent_invocation(
                identity.invocation.idempotency_key.clone(),
                OplogPayload::Inline(Box::new(AgentInvocationPayload::SaveSnapshot)),
                TraceId::generate(),
                Vec::new(),
                Vec::new(),
            ))
            .await;
        producer
            .append_session_record(StreamSessionRecordV1::Attached(
                StreamSessionAttachedRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: identity.invocation,
                    attachment_id,
                    attempt_id,
                    epoch: 1,
                    pending_invocation_oplog_index,
                },
            ))
            .await
            .unwrap();
        streams
            .activate_forwarded_mapping(attachment.clone(), mapping.clone(), producer.as_ref(), 110)
            .await
            .unwrap();
        assert_eq!(
            streams
                .topology_state(&attachment, Some(&mapping))
                .await
                .unwrap(),
            ConsumerAttachmentStatus::Active
        );
        assert_eq!(streams.handle(mapping.transport_stream_id), Some(handle));
    }

    #[test]
    async fn oversized_remote_result_is_rejected_before_any_session_write() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = DurableStreamProducer::load(
            oplog.clone(),
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let base_handle = producer
            .register(registration(
                &identity,
                StreamRegistrationCoordinateV1::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodResult,
                    recursive_value_path: Vec::new(),
                },
                StreamSourceKindV1::InvocationOutput,
            ))
            .await
            .unwrap()
            .value;
        let streams = DurableSessionStreams::new(producer, oplog.clone(), identity.invocation, []);
        let stream_count = MAX_NEW_STREAM_HANDLES_PER_VALUE + 1;
        let mut mappings = Vec::with_capacity(stream_count);
        let mut elements = Vec::with_capacity(stream_count);
        for position in 0..stream_count {
            let mut handle = base_handle.clone();
            handle.stream_id = golem_common::base_model::durable_stream::StreamId(Uuid::from_u128(
                10_000 + position as u128,
            ));
            mappings.push(StreamSessionMappingRecordV1 {
                transport_stream_id: position as u64,
                handle,
                role: SessionStreamRoleV1::Output,
            });
            elements.push(ProtoSchemaValue {
                value: Some(schema_value::Value::StreamReference(
                    SchemaValueStreamReference {
                        stream_id: position as u64,
                    },
                )),
            });
        }
        let value = ProtoSchemaValue {
            value: Some(schema_value::Value::ListValue(ListValue { elements })),
        };
        let element = SchemaType::u8();
        let root = SchemaType::list(SchemaType::stream(Some(element)));
        let graph = SchemaGraph::anonymous(root.clone());
        let before = oplog.current_oplog_index().await;

        let error = streams
            .materialize_remote_result(value, mappings, &graph, &root)
            .await
            .unwrap_err();

        assert!(error.contains("ResourceExhausted"));
        assert_eq!(oplog.current_oplog_index().await, before);
    }

    #[test]
    async fn remote_result_schema_mismatch_is_rejected_before_caller_mutation() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = DurableStreamProducer::load(
            oplog.clone(),
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let wrong_element = SchemaType::string();
        let wrong_graph = SchemaGraph::anonymous(wrong_element.clone());
        let mut request = registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::InvocationOutput,
        );
        request.element_schema_fingerprint =
            schema_fingerprint_v1(&wrong_graph, Some(&wrong_element)).unwrap();
        let handle = producer.register(request).await.unwrap().value;
        let streams = DurableSessionStreams::new(producer, oplog.clone(), identity.invocation, []);
        let expected_root = SchemaType::stream(Some(SchemaType::u32()));
        let expected_graph = SchemaGraph::anonymous(expected_root.clone());
        let value = ProtoSchemaValue {
            value: Some(schema_value::Value::StreamReference(
                SchemaValueStreamReference { stream_id: 7 },
            )),
        };
        let before = oplog.current_oplog_index().await;

        let error = streams
            .materialize_remote_result(
                value,
                vec![StreamSessionMappingRecordV1 {
                    transport_stream_id: 7,
                    handle: handle.clone(),
                    role: SessionStreamRoleV1::Output,
                }],
                &expected_graph,
                &expected_root,
            )
            .await
            .unwrap_err();

        assert!(error.contains("wrong schema fingerprint"));
        assert_eq!(oplog.current_oplog_index().await, before);
        assert!(
            streams
                .mapping_for_handle(&handle, SessionStreamRoleV1::Output)
                .is_none()
        );
        assert!(streams.remote_result_record().await.unwrap().is_none());
    }

    #[test]
    async fn remote_result_schema_validation_accepts_a_stream_in_union_branch_one() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = DurableStreamProducer::load(
            oplog.clone(),
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let root = union_with_stream_in_second_branch();
        let graph = SchemaGraph::anonymous(root.clone());
        let element = SchemaType::u32();
        let mut request = registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: vec![
                    StreamValuePathStepV1::UnionBranch(1),
                    StreamValuePathStepV1::RecordField(1),
                ],
            },
            StreamSourceKindV1::InvocationOutput,
        );
        request.element_schema_fingerprint = schema_fingerprint_v1(&graph, Some(&element)).unwrap();
        let handle = producer.register(request).await.unwrap().value;
        let streams = DurableSessionStreams::new(producer, oplog.clone(), identity.invocation, [])
            .with_consumer_journal(Arc::new(TestConsumerJournal(oplog)));
        let value = encode_recursive_stream_value_with_schema(
            &stream_union_value(SchemaValueStream::from_host_endpoint(())),
            &graph,
            &root,
            |_, path| {
                assert_eq!(
                    path,
                    [
                        StreamValuePathStepV1::UnionBranch(1),
                        StreamValuePathStepV1::RecordField(1),
                    ]
                );
                Ok(11)
            },
        )
        .unwrap();

        let result = streams
            .materialize_remote_result(
                value,
                vec![StreamSessionMappingRecordV1 {
                    transport_stream_id: 11,
                    handle: handle.clone(),
                    role: SessionStreamRoleV1::Output,
                }],
                &graph,
                &root,
            )
            .await
            .unwrap();

        let SchemaValue::Union(result) = result else {
            panic!("expected union result")
        };
        assert_eq!(result.tag, "stream");
        assert_eq!(
            streams
                .mapping_for_handle(&handle, SessionStreamRoleV1::Output)
                .unwrap()
                .role,
            SessionStreamRoleV1::Output
        );
    }

    #[test]
    async fn output_catch_up_persists_a_missing_nested_transport_mapping_before_emitting() {
        let identity = identity();
        let attachment_id = AttachmentId::primary(
            identity.environment_id,
            &identity.agent_id,
            &identity.invocation.idempotency_key,
        )
        .unwrap();
        let oplog = Arc::new(TestOplog::default());
        let producer = DurableStreamProducer::load(
            oplog.clone(),
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let session_key = identity.invocation.clone();
        let mut root_request = registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: session_key.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::InvocationOutput,
        );
        root_request.session_mapping = Some(StreamSessionMappingV1 {
            session_key: session_key.clone(),
            attachment_id,
            role: SessionStreamRoleV1::Output,
        });
        let root = producer.register(root_request).await.unwrap().value;
        let nested_request = registration(
            &identity,
            StreamRegistrationCoordinateV1::Nested {
                parent_stream_id: root.stream_id,
                parent_producer_sequence: 0,
                recursive_value_path: Vec::new(),
            },
            StreamSourceKindV1::Nested,
        );
        let canonical_value = ProtoSchemaValue {
            value: Some(schema_value::Value::StreamReference(
                SchemaValueStreamReference { stream_id: 0 },
            )),
        };
        let root_written = producer
            .write_items_with_nested(
                root.stream_id,
                0,
                StreamItemsPayloadV1::Values(vec![canonical_value.encode_to_vec()]),
                vec![nested_request],
            )
            .await
            .unwrap();
        producer
            .end(root.stream_id, 1, StreamEndResultV1::Ok)
            .await
            .unwrap();
        let nested = producer.nested_handles(root.stream_id, 0).await.unwrap()[0].clone();
        let nested_written = producer
            .write_items(
                nested.stream_id,
                0,
                StreamItemsPayloadV1::PackedU8(vec![10, 11]),
            )
            .await
            .unwrap();
        producer
            .end(nested.stream_id, 2, StreamEndResultV1::Ok)
            .await
            .unwrap();
        let attempt_id = AttemptId::fresh();
        producer
            .append_session_record(StreamSessionRecordV1::Prepared(
                StreamSessionPreparedRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    attempt: StartAttemptDescriptorV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: session_key.clone(),
                        attachment_id,
                        expected_callee_fingerprint: identity.fingerprint,
                        attempt_id,
                        invocation: PersistedStreamInvocationDescriptorV1 {
                            format_version: DURABLE_STREAM_FORMAT_VERSION,
                            session_key: session_key.clone(),
                            target_component_revision: ComponentRevision::INITIAL,
                            method_name: "produce".to_string(),
                            invocation_value: vec![1],
                            stream_handles: Vec::new(),
                            execution_config: vec![2],
                            effective_identity: vec![3],
                        },
                        effective_identity: vec![3],
                        live_join_buffer_events: 8,
                    },
                    stream_mappings: Vec::new(),
                },
            ))
            .await
            .unwrap();
        let pending_invocation_oplog_index = oplog
            .add(OplogEntry::pending_agent_invocation(
                session_key.idempotency_key.clone(),
                OplogPayload::Inline(Box::new(AgentInvocationPayload::SaveSnapshot)),
                TraceId::generate(),
                Vec::new(),
                Vec::new(),
            ))
            .await;
        let streams = DurableSessionStreams::new(
            producer.clone(),
            oplog.clone(),
            session_key.clone(),
            [(7, root.clone(), SessionStreamRoleV1::Output)],
        )
        .with_consumer_journal(Arc::new(TestConsumerJournal(oplog.clone())));
        streams
            .append_record(StreamSessionRecordV1::Attached(
                golem_common::base_model::durable_stream::StreamSessionAttachedRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: session_key.clone(),
                    attachment_id,
                    attempt_id,
                    epoch: 1,
                    pending_invocation_oplog_index,
                },
            ))
            .await;
        let streams = streams.with_attachment(1, attempt_id);
        assert!(
            streams
                .mapping_for_handle(&nested, SessionStreamRoleV1::Output)
                .is_none()
        );
        let (responses, mut receiver) = mpsc::channel(4);
        let discovered = streams
            .pump_output_stream_from(7, root.clone(), None, &responses)
            .await
            .unwrap();
        let [(nested_transport_stream_id, discovered_nested)] = discovered.as_slice() else {
            panic!("catch-up must discover exactly one nested output stream")
        };
        assert_eq!(discovered_nested, &nested);
        let item = receiver.recv().await.unwrap();
        let Some(invocation_response::Response::OutputItem(item)) = item.response else {
            panic!("catch-up must emit the enclosing output item first")
        };
        assert_eq!(item.new_stream_mappings.len(), 1);
        assert_eq!(
            item.new_stream_mappings[0].transport_stream_id,
            *nested_transport_stream_id
        );

        let restarted = DurableSessionStreams::new(
            producer,
            oplog.clone(),
            session_key,
            [(7, root.clone(), SessionStreamRoleV1::Output)],
        )
        .with_consumer_journal(Arc::new(TestConsumerJournal(oplog)))
        .with_attachment(1, attempt_id);
        restarted.recover_session_mappings().await.unwrap();
        assert_eq!(
            restarted
                .mapping_for_handle(&nested, SessionStreamRoleV1::Output)
                .unwrap()
                .transport_stream_id,
            *nested_transport_stream_id
        );
        let nested_transport_stream_id = *nested_transport_stream_id;
        let cursors = HashMap::from([
            (root.stream_id, Some(root_written.value[0])),
            (nested.stream_id, Some(nested_written.value[0])),
        ]);
        let (responses, mut receiver) = mpsc::channel(8);
        restarted
            .pump_output_streams_from(&cursors, &[7, nested_transport_stream_id], &responses)
            .await
            .unwrap();
        drop(responses);
        let mut replayed_nested_offsets = Vec::new();
        let mut ended_streams = HashSet::new();
        while let Some(response) = receiver.recv().await {
            match response.response {
                Some(invocation_response::Response::OutputItem(item)) => {
                    assert_eq!(item.transport_stream_id, nested_transport_stream_id);
                    replayed_nested_offsets.push(item.durable_offset);
                }
                Some(invocation_response::Response::OutputEnd(end)) => {
                    ended_streams.insert(end.transport_stream_id);
                }
                other => panic!("unexpected resumed nested output response: {other:?}"),
            }
        }
        assert_eq!(
            replayed_nested_offsets,
            vec![nested_written.value[1].as_bytes().to_vec()]
        );
        assert_eq!(
            ended_streams,
            HashSet::from([7, nested_transport_stream_id])
        );
    }

    #[test]
    async fn caller_attempt_is_random_v4_persisted_and_reused_after_restart() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = DurableStreamProducer::load(
            oplog.clone(),
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let streams = DurableSessionStreams::new(
            producer.clone(),
            oplog.clone(),
            identity.invocation.clone(),
            [],
        )
        .with_consumer_journal(Arc::new(TestConsumerJournal(oplog.clone())));

        let attempt = streams.caller_attempt_id().await.unwrap();
        assert_eq!(attempt.0.get_version(), Some(uuid::Version::Random));
        assert!(!attempt.0.is_nil());
        assert_eq!(streams.caller_attempt_id().await.unwrap(), attempt);

        let restarted =
            DurableSessionStreams::new(producer, oplog.clone(), identity.invocation, [])
                .with_consumer_journal(Arc::new(TestConsumerJournal(oplog)));
        assert_eq!(restarted.caller_attempt_id().await.unwrap(), attempt);
    }

    #[test]
    async fn forwarded_root_input_and_result_preserve_the_complete_handle() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = DurableStreamProducer::load(
            oplog.clone(),
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let element_type = SchemaType::u32();
        let graph = SchemaGraph::anonymous(element_type.clone());
        let fingerprint = schema_fingerprint_v1(&graph, Some(&element_type)).unwrap();
        let source_coordinate = StreamRegistrationCoordinateV1::Root {
            invocation_id: identity.invocation.clone(),
            root_kind: StreamRootKindV1::MethodResult,
            recursive_value_path: vec![StreamValuePathStepV1::ListElement(7)],
        };
        let mut request = registration(
            &identity,
            source_coordinate,
            StreamSourceKindV1::InvocationOutput,
        );
        request.element_schema_fingerprint = fingerprint.clone();
        let original = producer.register(request).await.unwrap().value;
        let streams = DurableSessionStreams::new(
            producer.clone(),
            oplog.clone(),
            identity.invocation.clone(),
            [],
        )
        .with_consumer_journal(Arc::new(TestConsumerJournal(oplog.clone())));
        let root_type = SchemaType::stream(Some(element_type));

        let input = SchemaValue::Stream(SchemaValueStream::from_host_endpoint(
            ForwardedDurableInput {
                handle: original.clone(),
            },
        ));
        let (_, input_mappings) = streams
            .materialize_agent_input(
                &input,
                &graph,
                &root_type,
                golem_common::model::component::ComponentRevision::INITIAL,
            )
            .await
            .unwrap();
        assert_eq!(input_mappings.len(), 1);
        assert_eq!(input_mappings[0].handle, original);
        assert!(
            producer
                .handle_for_coordinate(&StreamRegistrationCoordinateV1::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodInput,
                    recursive_value_path: Vec::new(),
                })
                .await
                .is_none()
        );

        let result = SchemaValue::Stream(SchemaValueStream::from_host_endpoint(
            ForwardedDurableInput {
                handle: original.clone(),
            },
        ));
        streams
            .materialize_result(
                result,
                &graph,
                &root_type,
                golem_common::model::component::ComponentRevision::INITIAL,
            )
            .await
            .unwrap();
        let persisted = streams.remote_result_record().await.unwrap().unwrap();
        assert_eq!(persisted.output_streams, vec![original.clone()]);
        assert_eq!(persisted.stream_mappings[0].handle, original);
    }

    #[test]
    async fn forwarded_nested_stream_is_persisted_by_full_handle_without_re_registration() {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = DurableStreamProducer::load(
            oplog.clone(),
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let forwarded = producer
            .register(registration(
                &identity,
                StreamRegistrationCoordinateV1::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodInput,
                    recursive_value_path: Vec::new(),
                },
                StreamSourceKindV1::AgentHostedInput,
            ))
            .await
            .unwrap()
            .value;
        let parent = producer
            .register(registration(
                &identity,
                StreamRegistrationCoordinateV1::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodResult,
                    recursive_value_path: Vec::new(),
                },
                StreamSourceKindV1::InvocationOutput,
            ))
            .await
            .unwrap()
            .value;
        let streams =
            DurableSessionStreams::new(producer.clone(), oplog.clone(), identity.invocation, [])
                .with_consumer_journal(Arc::new(TestConsumerJournal(oplog)));
        streams
            .append_mapping_once(StreamSessionMappingRecordV1 {
                transport_stream_id: 3,
                handle: forwarded.clone(),
                role: SessionStreamRoleV1::Output,
            })
            .await
            .unwrap();
        let value = ProtoSchemaValue {
            value: Some(schema_value::Value::StreamReference(
                SchemaValueStreamReference { stream_id: 0 },
            )),
        };
        producer
            .write_items_with_nested_sources(
                parent.stream_id,
                0,
                StreamItemsPayloadV1::Values(vec![value.encode_to_vec()]),
                vec![NestedStreamWriteV1::Forward(forwarded.clone())],
            )
            .await
            .unwrap();

        let mut reader = producer.catch_up(parent, None).await.unwrap();
        let event = reader.next().await.unwrap().unwrap();
        assert_eq!(event.nested_handles, vec![forwarded]);
    }
}
