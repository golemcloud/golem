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
    AttachedStreamSegmentSource, CommittedProducerStreamEventPayloadV1,
    CommittedProducerStreamEventV1, ConsumerAttachmentStatus, DurableCatchUpReader,
    DurableStreamProducer, DurableStreamProducerError, NestedStreamWriteV1,
    ProducerOutputRegistrationV1, ProducerOutputSourceV1, ProducerRegistrationRequestV1,
    RoutedAttachedStreamSegmentSource, RoutedStreamAttachmentControl,
    StreamAttachmentConsumerProbe, StreamAttachmentControl, StreamSegmentSource,
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
    DurableStreamHandle, DurableStreamMapping, InputStreamHighWater, InvocationResponse,
    OutputStreamEnd, OutputStreamError, OutputStreamItem, StreamCancel, StreamInvocationIdentity,
    StreamMappingRole, invocation_response,
};
use golem_common::base_model::durable_stream::{
    AttachmentId, AttemptId, DURABLE_STREAM_FORMAT_VERSION, DurableStreamHandleV1,
    InputStreamHighWaterV1, MAX_NEW_STREAM_HANDLES_PER_VALUE, MAX_PACKED_U8_STREAM_ITEM_SIZE,
    SessionStreamRoleV1, StreamAttachmentKeyV1, StreamCallerAttemptRecordV1, StreamCancelReasonV1,
    StreamCancelRoleV1, StreamConsumerCancelIntentRecordV1, StreamConsumerItemValueRecordV1,
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
pub(crate) trait DurableStreamConsumerJournal: Send + Sync {
    async fn commit(&self) -> Result<(), String>;
    async fn committed_finished_index(
        &self,
        session: &StreamSessionKeyV1,
    ) -> Result<Option<OplogIndex>, String>;
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

#[derive(Clone, Default, desert_rust::BinaryCodec)]
pub struct SessionControlMetadata {
    pub(crate) covered_through: OplogIndex,
    pub(crate) recovery_slot: Option<u64>,
    pub(crate) prepared: Option<OplogIndex>,
    pub(crate) initial_attached: Option<OplogIndex>,
    malformed_record: bool,
    explicit_mappings: HashSet<(u64, DurableStreamHandleV1, SessionStreamRoleV1)>,
    persisted_mappings: HashSet<(u64, DurableStreamHandleV1, SessionStreamRoleV1)>,
    recoverable_mappings: Vec<(OplogIndex, StreamSessionMappingRecordV1)>,
    acceptance_mappings: Vec<StreamSessionMappingRecordV1>,
    caller_attempt: Option<AttemptId>,
    caller_attempt_conflict: bool,
    pub(crate) invocation_result: Option<OplogIndex>,
    pub(crate) finished: Option<OplogIndex>,
    root_outputs: Vec<u64>,
    topology_epoch: Option<u64>,
    topologies: HashMap<
        (
            AttachmentId,
            golem_common::model::StreamId,
            u64,
            SessionStreamRoleV1,
        ),
        SessionTopologyMetadata,
    >,
    visible_mappings: HashSet<(u64, DurableStreamHandleV1, SessionStreamRoleV1)>,
    topology_error: Option<String>,
    finalized_attachments:
        HashMap<(AttachmentId, golem_common::model::StreamId), StreamAttachmentKeyV1>,
    closed_consumer_streams: HashSet<golem_common::model::StreamId>,
    cancel_intents: HashMap<golem_common::model::StreamId, StreamConsumerCancelIntentRecordV1>,
    pub(crate) consumer_record_counts: HashMap<golem_common::model::StreamId, u64>,
    pub(crate) consumer_deleting:
        Option<golem_common::model::durable_stream::StreamConsumerDeletingRecordV1>,
}

#[derive(Clone, desert_rust::BinaryCodec)]
struct SessionTopologyMetadata {
    attachment: StreamAttachmentKeyV1,
    mapping: StreamSessionMappingRecordV1,
    active: bool,
    prepared_index: Option<OplogIndex>,
    activated_index: Option<OplogIndex>,
    repeated_activation_index: Option<OplogIndex>,
}

impl SessionControlMetadata {
    pub(crate) fn acceptance_mappings(&self) -> Result<Vec<StreamSessionMappingRecordV1>, String> {
        if self.malformed_record {
            return Err("unsupported or malformed durable Stream Session record version".into());
        }
        if let Some(error) = &self.topology_error {
            return Err(error.clone());
        }
        Ok(self.acceptance_mappings.clone())
    }

    pub(crate) fn needs_topology_recovery(
        &self,
        owner: &golem_common::model::OwnedAgentId,
        key: &StreamSessionKeyV1,
    ) -> bool {
        self.malformed_record
            || self.topology_error.is_some()
            || (self.prepared.is_some() && self.finished.is_none())
            || self.topologies.values().any(|topology| {
                let local = key.callee_environment_id == owner.environment_id
                    && key.callee == owner.agent_id
                    && key.callee_fingerprint == topology.attachment.expected_consumer_fingerprint;
                !(local && self.finished.is_some())
                    && !self
                        .closed_consumer_streams
                        .contains(&topology.attachment.stream_id)
                    && self.finalized_attachments.get(&(
                        topology.attachment.attachment_id,
                        topology.attachment.stream_id,
                    )) != Some(&topology.attachment)
            })
    }

    pub(crate) fn recovery_topologies(
        &self,
        owner: &golem_common::model::OwnedAgentId,
        key: &StreamSessionKeyV1,
    ) -> Result<Vec<(StreamAttachmentKeyV1, StreamSessionMappingRecordV1)>, String> {
        if self.malformed_record {
            return Err("unsupported or malformed durable Stream Session record version".into());
        }
        if let Some(error) = &self.topology_error {
            return Err(error.clone());
        }
        Ok(self
            .topologies
            .values()
            .filter(|topology| {
                let local = key.callee_environment_id == owner.environment_id
                    && key.callee == owner.agent_id
                    && key.callee_fingerprint == topology.attachment.expected_consumer_fingerprint;
                !(local && self.finished.is_some())
                    && !self
                        .closed_consumer_streams
                        .contains(&topology.attachment.stream_id)
                    && (!local
                        || self
                            .topology_epoch
                            .is_none_or(|epoch| epoch == topology.attachment.epoch))
                    && self.finalized_attachments.get(&(
                        topology.attachment.attachment_id,
                        topology.attachment.stream_id,
                    )) != Some(&topology.attachment)
            })
            .map(|topology| (topology.attachment.clone(), topology.mapping.clone()))
            .collect())
    }

    pub(crate) fn topology_status(
        &self,
        attachment: &StreamAttachmentKeyV1,
        expected_mapping: Option<&StreamSessionMappingRecordV1>,
    ) -> Result<ConsumerAttachmentStatus, String> {
        if self.malformed_record {
            return Err("unsupported or malformed durable Stream Session record version".into());
        }
        if let Some(error) = &self.topology_error {
            return Err(error.clone());
        }
        let mut events = Vec::new();
        for topology in self
            .topologies
            .values()
            .filter(|topology| same_attachment_slot(&topology.attachment, attachment))
        {
            if topology.attachment.epoch < attachment.epoch {
                continue;
            }
            if topology.attachment != *attachment {
                return Ok(attachment_mismatch_status(&topology.attachment, attachment));
            }
            if expected_mapping.is_some_and(|mapping| mapping != &topology.mapping) {
                continue;
            }
            events.extend(topology.prepared_index.map(|index| (index, false)));
            events.extend(topology.activated_index.map(|index| (index, true)));
            events.extend(
                topology
                    .repeated_activation_index
                    .map(|index| (index, true)),
            );
        }
        events.sort_unstable_by_key(|(index, _)| *index);
        let mut state = ConsumerAttachmentStatus::Missing;
        for (_, active) in events {
            if !active {
                if state == ConsumerAttachmentStatus::Missing {
                    state = ConsumerAttachmentStatus::Prepared;
                }
            } else if !(expected_mapping.is_none() && state == ConsumerAttachmentStatus::Active) {
                if state != ConsumerAttachmentStatus::Prepared {
                    return Err("durable topology activation has no matching preparation".into());
                }
                state = ConsumerAttachmentStatus::Active;
            }
        }
        Ok(state)
    }

    pub(crate) fn apply(
        &mut self,
        index: OplogIndex,
        key: &StreamSessionKeyV1,
        record: &StreamSessionRecordV1,
    ) {
        self.malformed_record |= !record.has_supported_format();
        if let StreamSessionRecordV1::ConsumerDeleting(record) = record {
            self.consumer_deleting = Some(record.clone());
        }
        if let StreamSessionRecordV1::ConsumerCancelIntent(record) = record
            && &record.session_key == key
        {
            self.cancel_intents
                .entry(record.stream_id)
                .or_insert_with(|| record.clone());
        }
        if let StreamSessionRecordV1::Prepared(record) = record
            && &record.attempt.session_key == key
        {
            if self.prepared.is_some() {
                self.topology_error.get_or_insert_with(|| {
                    "durable Stream Session contains multiple Prepared records".into()
                });
            } else {
                self.prepared = Some(index);
            }
        }
        if let StreamSessionRecordV1::AttachmentFinalized(record) = record
            && &record.key.session_key == key
        {
            let slot = (record.key.attachment_id, record.key.stream_id);
            if self
                .finalized_attachments
                .get(&slot)
                .is_none_or(|old| old.epoch <= record.key.epoch)
            {
                self.finalized_attachments.insert(slot, record.key.clone());
            }
        }
        let consumer_stream = match record {
            StreamSessionRecordV1::ConsumerItemValue(record) if &record.session_key == key => {
                Some(record.stream_id)
            }
            StreamSessionRecordV1::ConsumerTerminal(record) if &record.session_key == key => {
                Some(record.stream_id)
            }
            StreamSessionRecordV1::SourceUnavailable(record) if &record.key.session_key == key => {
                Some(record.key.stream_id)
            }
            _ => None,
        };
        if let Some(stream) = consumer_stream {
            *self.consumer_record_counts.entry(stream).or_default() += 1;
            if matches!(
                record,
                StreamSessionRecordV1::ConsumerTerminal(_)
                    | StreamSessionRecordV1::SourceUnavailable(_)
            ) {
                self.closed_consumer_streams.insert(stream);
            }
        }
        match record {
            StreamSessionRecordV1::Attached(record) if &record.session_key == key => {
                if self.initial_attached.is_some() {
                    self.topology_error.get_or_insert_with(|| {
                        "durable Stream Session contains multiple Attached records".into()
                    });
                } else {
                    self.initial_attached = Some(index);
                }
                self.topology_epoch = Some(record.epoch);
            }
            StreamSessionRecordV1::ResumeAttempt(record) if &record.attempt.session_key == key => {
                self.topology_epoch = Some(record.accepted_epoch);
            }
            _ => {}
        }
        let topology = match record {
            StreamSessionRecordV1::TopologyPrepared(record) if &record.session_key == key => {
                Some((&record.attachment, &record.mapping, false))
            }
            StreamSessionRecordV1::TopologyActivated(record) if &record.session_key == key => {
                Some((&record.attachment, &record.mapping, true))
            }
            _ => None,
        };
        if let Some((attachment, mapping, active)) = topology {
            let slot = (
                attachment.attachment_id,
                attachment.stream_id,
                mapping.transport_stream_id,
                mapping.role,
            );
            match self.topologies.get_mut(&slot) {
                Some(existing)
                    if &existing.attachment != attachment || &existing.mapping != mapping =>
                {
                    if attachment.epoch > existing.attachment.epoch {
                        if active {
                            self.topology_error.get_or_insert_with(|| {
                                "durable topology activation has no matching preparation".into()
                            });
                        }
                        *existing = SessionTopologyMetadata {
                            attachment: attachment.clone(),
                            mapping: mapping.clone(),
                            active,
                            prepared_index: (!active).then_some(index),
                            activated_index: active.then_some(index),
                            repeated_activation_index: None,
                        };
                    } else {
                        self.topology_error.get_or_insert_with(|| {
                            "conflicting durable topology preparation or activation".into()
                        });
                    }
                }
                Some(existing) => {
                    existing.active |= active;
                    if active {
                        if existing.activated_index.is_some() {
                            existing.repeated_activation_index.get_or_insert(index);
                        } else {
                            existing.activated_index = Some(index);
                        }
                    } else {
                        existing.prepared_index.get_or_insert(index);
                    }
                }
                None => {
                    if active {
                        self.topology_error.get_or_insert_with(|| {
                            "durable topology activation has no matching preparation".into()
                        });
                    }
                    self.topologies.insert(
                        slot,
                        SessionTopologyMetadata {
                            attachment: attachment.clone(),
                            mapping: mapping.clone(),
                            active,
                            prepared_index: (!active).then_some(index),
                            activated_index: active.then_some(index),
                            repeated_activation_index: None,
                        },
                    );
                }
            }
        }
        let mappings: &[StreamSessionMappingRecordV1] = match record {
            StreamSessionRecordV1::CallerAttempt(record) if &record.session_key == key => {
                if self
                    .caller_attempt
                    .is_some_and(|attempt| attempt != record.attempt_id)
                {
                    self.caller_attempt_conflict = true;
                }
                self.caller_attempt.get_or_insert(record.attempt_id);
                &[]
            }
            StreamSessionRecordV1::Mapping(record) if &record.session_key == key => {
                self.explicit_mappings.insert((
                    record.mapping.transport_stream_id,
                    record.mapping.handle.clone(),
                    record.mapping.role,
                ));
                std::slice::from_ref(&record.mapping)
            }
            StreamSessionRecordV1::Prepared(record) if &record.attempt.session_key == key => {
                &record.stream_mappings
            }
            StreamSessionRecordV1::TopologyPrepared(record) if &record.session_key == key => {
                std::slice::from_ref(&record.mapping)
            }
            StreamSessionRecordV1::TopologyActivated(record) if &record.session_key == key => {
                std::slice::from_ref(&record.mapping)
            }
            StreamSessionRecordV1::ConsumerItemValue(record) if &record.session_key == key => {
                &record.recursive_mappings
            }
            StreamSessionRecordV1::InvocationResult(record) if &record.session_key == key => {
                self.invocation_result.get_or_insert(index);
                for mapping in &record.stream_mappings {
                    if mapping.role == SessionStreamRoleV1::Output
                        && !self.root_outputs.contains(&mapping.transport_stream_id)
                    {
                        self.root_outputs.push(mapping.transport_stream_id);
                    }
                }
                &record.stream_mappings
            }
            StreamSessionRecordV1::Finished(record) if &record.session_key == key => {
                self.finished.get_or_insert(index);
                &[]
            }
            _ => &[],
        };
        for mapping in mappings {
            if !self.acceptance_mappings.contains(mapping) {
                self.acceptance_mappings.push(mapping.clone());
            }
        }
        if matches!(
            record,
            StreamSessionRecordV1::Prepared(_)
                | StreamSessionRecordV1::Mapping(_)
                | StreamSessionRecordV1::InvocationResult(_)
        ) {
            self.visible_mappings.extend(mappings.iter().map(|mapping| {
                (
                    mapping.transport_stream_id,
                    mapping.handle.clone(),
                    mapping.role,
                )
            }));
        }
        if matches!(
            record,
            StreamSessionRecordV1::Mapping(_) | StreamSessionRecordV1::InvocationResult(_)
        ) {
            for mapping in mappings {
                if !self
                    .recoverable_mappings
                    .iter()
                    .any(|(_, existing)| existing == mapping)
                {
                    self.recoverable_mappings.push((index, mapping.clone()));
                }
            }
        }
        self.persisted_mappings
            .extend(mappings.iter().map(|mapping| {
                (
                    mapping.transport_stream_id,
                    mapping.handle.clone(),
                    mapping.role,
                )
            }));
        self.covered_through = index;
    }
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

fn stream_cancel_reason_to_proto(
    reason: StreamCancelReasonV1,
) -> golem_api_grpc::proto::golem::worker::StreamCancelReason {
    match reason {
        StreamCancelReasonV1::Cancelled => {
            golem_api_grpc::proto::golem::worker::StreamCancelReason::Cancelled
        }
        StreamCancelReasonV1::GuestDrop => {
            golem_api_grpc::proto::golem::worker::StreamCancelReason::ConsumerDrop
        }
        StreamCancelReasonV1::Protocol => {
            golem_api_grpc::proto::golem::worker::StreamCancelReason::Protocol
        }
        StreamCancelReasonV1::InvocationFailed => {
            golem_api_grpc::proto::golem::worker::StreamCancelReason::InvocationFailed
        }
        StreamCancelReasonV1::SourceUnavailable => {
            golem_api_grpc::proto::golem::worker::StreamCancelReason::SourceUnavailable
        }
        StreamCancelReasonV1::ProducerDeleting => {
            golem_api_grpc::proto::golem::worker::StreamCancelReason::ProducerDeleting
        }
    }
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

    pub(crate) fn with_response_lease(
        mut self,
        lease: Option<Arc<crate::worker::EphemeralResponseLease>>,
    ) -> Self {
        self.response_lease = lease;
        self
    }

    pub(crate) fn response_lease(&self) -> Option<Arc<crate::worker::EphemeralResponseLease>> {
        self.response_lease.clone()
    }

    pub(crate) fn with_attachment(mut self, epoch: u64, attempt_id: AttemptId) -> Self {
        self.recovered_mappings_through = Arc::new(Mutex::new(OplogIndex::NONE));
        self.attachment_epoch = epoch;
        self.attachment_attempt_id = Some(attempt_id);
        self
    }

    pub(crate) fn with_consumer_invocation(
        mut self,
        consumer_invocation: StreamInvocationIdV1,
    ) -> Self {
        self.recovered_mappings_through = Arc::new(Mutex::new(OplogIndex::NONE));
        self.consumer_invocation = consumer_invocation;
        self
    }

    pub(crate) fn with_entity_parent_start_index(
        mut self,
        entity_parent_start_index: Option<OplogIndex>,
    ) -> Self {
        self.entity_parent_start_index = entity_parent_start_index;
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

    pub(crate) async fn detach_current(&self) -> Result<bool, String> {
        let session = self.clone();
        self.producer
            .run_lifecycle(
                0,
                move |_| async move { session.detach_current_owned().await },
            )
            .await
    }

    async fn detach_current_owned(&self) -> Result<bool, String> {
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
        let memory = golem_common::serialization::serialize(&record)?.len();
        let session = self.clone();
        self.producer
            .run_lifecycle(memory, move |_| async move {
                session.commit_resume_attempt_owned(record).await
            })
            .await
    }

    async fn commit_resume_attempt_owned(
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
            .append_session_record_attributed(
                self.entity_parent_start_index,
                StreamSessionRecordV1::ResumeAttempt(record),
            )
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
            .append_session_record_attributed(self.entity_parent_start_index, record)
            .await
            .expect("internally generated durable session record is valid");
    }

    async fn try_append_record(&self, record: StreamSessionRecordV1) -> Result<(), String> {
        self.producer
            .append_session_record_attributed(self.entity_parent_start_index, record)
            .await
            .map_err(|error| error.to_string())
    }

    pub(crate) async fn caller_attempt_id(&self) -> Result<AttemptId, String> {
        let session = self.clone();
        self.producer
            .run_owned(0, move |_| async move {
                session.caller_attempt_id_owned().await
            })
            .await
    }

    async fn caller_attempt_id_owned(&self) -> Result<AttemptId, String> {
        let _guard = self.session_lock.lock().await;
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

    async fn current_control_metadata(
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

    async fn session_record_at(&self, index: OplogIndex) -> Result<StreamSessionRecordV1, String> {
        let OplogEntry::StreamSession { record, .. } = self.oplog.read(index).await else {
            return Err("durable session metadata points at a non-session record".into());
        };
        self.download_record(record).await
    }

    async fn append_mapping_once(
        &self,
        mapping: StreamSessionMappingRecordV1,
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
        producer_control: Arc<dyn StreamAttachmentControl + Send + Sync>,
        now_millis: u64,
    ) -> Result<(), String> {
        let memory = golem_common::serialization::serialize(&attachment)?.len()
            + golem_common::serialization::serialize(&mapping)?.len();
        let session = self.clone();
        self.producer
            .run_owned(memory, move |_| async move {
                let _session_guard = session.session_lock.lock().await;
                session
                    .activate_forwarded_mapping_under_lock(
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
        self.producer
            .remote_until_retired(
                producer_control.prepare_attachment(attachment.clone(), now_millis),
            )
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
        self.producer
            .remote_until_retired(producer_control.activate_attachment(attachment, now_millis))
            .await
            .map_err(|error| error.to_string())?;
        self.append_mapping_once(mapping.clone()).await?;
        self.commit_consumer_journal().await?;
        self.insert_mapping(mapping.transport_stream_id, mapping.handle, mapping.role)
    }

    pub(crate) async fn require_local_session_attachment(
        &self,
        attachment: &StreamAttachmentKeyV1,
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

    pub(crate) async fn recover_nested_input_mappings(&self) -> Result<(), String> {
        self.recover_session_mappings().await
    }

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
            self.insert_mapping(mapping.transport_stream_id, mapping.handle, mapping.role)?;
        }
        *covered = horizon;
        Ok(())
    }

    pub(crate) async fn has_journaled_consumer_terminal(
        &self,
        mapping: &StreamSessionMappingRecordV1,
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
        mapping: &StreamSessionMappingRecordV1,
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
                    (handle.stream_id == cursor.stream_id && *role == SessionStreamRoleV1::Output)
                        .then_some(StreamSessionMappingRecordV1 {
                            transport_stream_id: *transport_stream_id,
                            handle: handle.clone(),
                            role: *role,
                        })
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
            Arc::new(control),
            Timestamp::now_utc().to_millis(),
        )
        .await
    }

    pub(crate) async fn prepare_foreign_mapping(
        &self,
        mapping: StreamSessionMappingRecordV1,
        epoch: u64,
    ) -> Result<(), String> {
        let memory = golem_common::serialization::serialize(&mapping)?.len();
        let session = self.clone();
        self.producer
            .run_owned(memory, move |_| async move {
                session.prepare_foreign_mapping_owned(mapping, epoch).await
            })
            .await
    }

    async fn prepare_foreign_mapping_owned(
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
        expected: &StreamSessionMappingRecordV1,
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
        handle: DurableStreamHandleV1,
        role: SessionStreamRoleV1,
    ) -> Result<StreamSessionMappingRecordV1, String> {
        let memory = golem_common::serialization::serialize(&handle)?.len();
        let session = self.clone();
        self.producer
            .run_owned(memory, move |_| async move {
                let _session_guard = session.session_lock.lock().await;
                session.ensure_nested_mapping_under_lock(handle, role).await
            })
            .await
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
            let attachment = self.attachment_key(&mapping.handle, self.attachment_epoch)?;
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
        Option<(
            u64,
            u64,
            golem_common::model::durable_stream::StreamOffsetV1,
            Vec<StreamSessionMappingRecordV1>,
        )>,
        String,
    > {
        let memory = DurableStreamProducer::retained_payload_bytes(&payload)?;
        let session = self.clone();
        self.producer
            .run_owned(memory, move |_| async move {
                session
                    .write_input_owned(transport_stream_id, first_sequence, payload)
                    .await
            })
            .await
    }

    async fn write_input_owned(
        &self,
        transport_stream_id: u64,
        first_sequence: u64,
        payload: StreamItemsPayloadV1,
    ) -> Result<
        Option<(
            u64,
            u64,
            golem_common::model::durable_stream::StreamOffsetV1,
            Vec<StreamSessionMappingRecordV1>,
        )>,
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
                            .map_err(|error| error.to_string())?
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
                        entity_parent_start_index: self.entity_parent_start_index,
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
        let outcome = match self
            .producer
            .write_items_with_nested(
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
        Ok(Some((
            highest_contiguous_sequence,
            logical_item_count,
            resulting_offset,
            nested_mappings,
        )))
    }

    pub(crate) async fn end_input(
        &self,
        transport_stream_id: u64,
        sequence: u64,
    ) -> Result<Option<golem_common::model::durable_stream::StreamOffsetV1>, String> {
        let session = self.clone();
        self.producer
            .run_owned(0, move |_| async move {
                session.end_input_owned(transport_stream_id, sequence).await
            })
            .await
    }

    async fn end_input_owned(
        &self,
        transport_stream_id: u64,
        sequence: u64,
    ) -> Result<Option<golem_common::model::durable_stream::StreamOffsetV1>, String> {
        let _session_guard = self.session_lock.lock().await;
        self.ensure_current_attachment().await?;
        let handle = self
            .handle(transport_stream_id)
            .ok_or_else(|| format!("unknown durable input stream {transport_stream_id}"))?;
        match self
            .producer
            .end(handle.stream_id, sequence, StreamEndResultV1::Ok)
            .await
        {
            Ok(outcome) => Ok(Some(outcome.value)),
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

    pub(crate) async fn cancel_stream(
        &self,
        transport_stream_id: u64,
        role: StreamCancelRoleV1,
        reason: StreamCancelReasonV1,
        details: Option<String>,
        expected_attachment_epoch: Option<u64>,
    ) -> Result<(), String> {
        let session = self.clone();
        self.producer
            .run_lifecycle(
                details.as_ref().map_or(0, String::len),
                move |_| async move {
                    session
                        .cancel_stream_owned(
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
        let locally_produced = self.producer.owns_handle_identity(&mapping.handle);
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
        let intent = StreamConsumerCancelIntentRecordV1 {
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
                self.append_record(StreamSessionRecordV1::ConsumerCancelIntent(intent.clone()))
                    .await;
                self.commit_consumer_journal().await?;
                intent
            }
        };
        if locally_produced {
            let pending = self
                .producer
                .commit_cancel_open(
                    mapping.handle.stream_id,
                    intent.role,
                    intent.reason,
                    intent.details,
                )
                .await
                .map_err(|error| error.to_string())?;
            drop(session_guard);
            if let Some(pending) = pending {
                self.producer
                    .publish_committed_cancellation(pending)
                    .await
                    .map_err(|error| error.to_string())?;
            }
        } else {
            drop(session_guard);
            let rpc = self.rpc.clone().ok_or_else(|| {
                "foreign durable stream cancellation routing is unavailable".to_string()
            })?;
            let auth_ctx = self.auth_ctx.clone().ok_or_else(|| {
                "foreign durable stream cancellation authorization is unavailable".to_string()
            })?;
            let key = self.attachment_key(&mapping.handle, intent.epoch)?;
            self.producer.defer_remote_cancellation(async move {
                RoutedStreamAttachmentControl::new(rpc, mapping, auth_ctx)
                    .cancel_stream(key, intent.role, intent.reason, intent.details)
                    .await
                    .map(|_| ())
            });
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
            .run_owned(retained_bytes, move |_| async move {
                session
                    .materialize_agent_input_owned(value, graph, root, component_revision)
                    .await
            })
            .await
    }

    async fn materialize_agent_input_owned(
        &self,
        value: SchemaValue,
        graph: SchemaGraph,
        root: SchemaType,
        component_revision: golem_common::model::component::ComponentRevision,
    ) -> Result<(ProtoSchemaValue, Vec<StreamSessionMappingRecordV1>), String> {
        struct PendingInput {
            path: Vec<StreamValuePathStepV1>,
            endpoint: Option<LiveStreamEndpoint>,
            forwarded_handle: Option<DurableStreamHandleV1>,
            element_type: SchemaType,
            element_schema_fingerprint: SchemaFingerprintV1,
        }

        let _session_guard = self.session_lock.lock().await;
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
                    entity_parent_start_index: self.entity_parent_start_index,
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
            .run_owned(retained_bytes, move |_| async move {
                let (result, drains) = session
                    .materialize_result_owned(value, graph, root, component_revision)
                    .await?;
                let drain = tokio::spawn(async move {
                    session
                        .drain_materialized_result(drains, Arc::new(drain_graph))
                        .await
                });
                Ok::<_, String>((result, drain))
            })
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
        value: SchemaValue,
        graph: SchemaGraph,
        root: SchemaType,
        component_revision: golem_common::model::component::ComponentRevision,
    ) -> Result<(SchemaValue, Vec<PendingOwnedStreamDrain>), String> {
        let session_guard = self.session_lock.lock().await;
        struct PendingOutput {
            path: Vec<StreamValuePathStepV1>,
            endpoint: Option<LiveStreamEndpoint>,
            forwarded_handle: Option<DurableStreamHandleV1>,
            element_type: SchemaType,
            element_schema_fingerprint: SchemaFingerprintV1,
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
                pending.push(PendingOutput {
                    path: path.to_vec(),
                    endpoint,
                    forwarded_handle,
                    element_type: element.cloned().unwrap_or_else(SchemaType::u8),
                    element_schema_fingerprint,
                });
                Ok(canonical_handle_index)
            })?;

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
                entity_parent_start_index: self.entity_parent_start_index,
                coordinate: StreamRegistrationCoordinateV1::Root {
                    invocation_id: self.session_key.clone(),
                    root_kind: StreamRootKindV1::MethodResult,
                    recursive_value_path: pending.path.clone(),
                },
                source_kind: StreamSourceKindV1::InvocationOutput,
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
                    self.mapping_for_handle(handle, SessionStreamRoleV1::Output)
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
                    .map_err(|error| error.to_string())?
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
        let mut requests = requests.into_iter();
        let outputs = pending
            .iter()
            .zip(&transport_stream_ids)
            .map(
                |(pending, &transport_stream_id)| ProducerOutputRegistrationV1 {
                    transport_stream_id,
                    source: match &pending.forwarded_handle {
                        Some(handle) => ProducerOutputSourceV1::Existing(handle.clone()),
                        None => ProducerOutputSourceV1::New(
                            requests
                                .next()
                                .expect("each owned output has a registration request"),
                        ),
                    },
                },
            )
            .collect::<Vec<_>>();
        let (owned_handles, _) = self
            .producer
            .register_result_streams(
                self.session_key.clone(),
                result_bytes,
                outputs,
                self.entity_parent_start_index,
            )
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
                self.attach_foreign_handle(
                    remote.handle,
                    SessionStreamRoleV1::Output,
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
        let index = self.current_control_metadata().await?.invocation_result;
        let Some(index) = index else {
            return Ok(None);
        };
        match self.session_record_at(index).await? {
            StreamSessionRecordV1::InvocationResult(record)
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
                                handle.stream_id,
                                event.offset,
                                StreamEndResultV1::ErrorContext(error.into_bytes()),
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
                                registration: ProducerRegistrationRequestV1 {
                                    entity_parent_start_index: self.entity_parent_start_index,
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
                    let payload = match packed_u8 {
                        Some(bytes) => StreamItemsPayloadV1::PackedU8(bytes),
                        None => StreamItemsPayloadV1::Values(vec![value.encode_to_vec()]),
                    };
                    match self
                        .producer
                        .write_items_with_nested_sources(
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
                                    .run_owned(memory, move |_| async move {
                                        let _session_guard = session.session_lock.lock().await;
                                        for (output, nested_handle) in
                                            nested_outputs.into_iter().zip(nested_handles)
                                        {
                                            let transport_stream_id = session
                                                .mapping_for_handle(&nested_handle, role)
                                                .map(|mapping| mapping.transport_stream_id)
                                                .map(Ok)
                                                .unwrap_or_else(|| {
                                                    session.allocate_transport_stream_id()
                                                })?;
                                            session.insert_mapping(
                                                transport_stream_id,
                                                nested_handle.clone(),
                                                role,
                                            )?;
                                            session
                                                .append_mapping_once(StreamSessionMappingRecordV1 {
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
                                        Ok::<_, String>(())
                                    })
                                    .await?;
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
        input_cancel_reason: StreamCancelReasonV1,
    ) -> Result<(), String> {
        if self.has_committed_finished().await? {
            return Ok(());
        }
        let outcome = async {
            let session_guard = self.session_lock.lock().await;
            self.validate_topology_complete().await?;
            drop(session_guard);
            self.producer
                .finish_session(
                    self.session_key.clone(),
                    self.entity_parent_start_index,
                    result,
                    input_cancel_reason,
                )
                .await
                .map_err(|error| error.to_string())
        }
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
            StreamSessionRecordV1::Finished(record) if record.session_key == self.session_key => {
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
            if !topology.active {
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
                self.insert_mapping(mapping.transport_stream_id, mapping.handle, mapping.role)?;
            }
            return Ok(Some((transport_value, proto_mappings)));
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
        let index = self.current_control_metadata().await?.finished;
        let Some(index) = index else {
            return Ok(None);
        };
        match self.session_record_at(index).await? {
            StreamSessionRecordV1::Finished(record) if record.session_key == self.session_key => {
                Ok(Some(record.result))
            }
            _ => Err("durable finished metadata points at a different record".into()),
        }
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
            Option<golem_common::model::durable_stream::StreamOffsetV1>,
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
            Option<golem_common::model::durable_stream::StreamOffsetV1>,
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
            Option<golem_common::model::durable_stream::StreamOffsetV1>,
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
        announced_high_waters: &HashMap<u64, InputStreamHighWaterV1>,
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
                    && handle.expected_producer_fingerprint == self.session_key.callee_fingerprint
                    && !announced_high_waters
                        .get(transport_stream_id)
                        .is_some_and(|high_water| high_water.terminal))
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
            .stream_reader(handle, after, SessionStreamRoleV1::Output)
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
                CommittedProducerStreamEventPayloadV1::Value(bytes) => {
                    let memory = bytes.len()
                        + golem_common::serialization::serialize(&event.nested_handles)?.len();
                    let session = self.clone();
                    let mapping_update = async move {
                        let _session_guard = session.session_lock.lock().await;
                        session.ensure_current_attachment().await?;
                        session.recover_session_mappings().await?;
                        let mut nested_streams = Vec::new();
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
                            let mapping = match session
                                .mapping_for_handle(&handle, SessionStreamRoleV1::Output)
                            {
                                Some(mapping) => mapping,
                                None => {
                                    let transport_stream_id =
                                        session.allocate_transport_stream_id()?;
                                    let mapping = StreamSessionMappingRecordV1 {
                                        transport_stream_id,
                                        handle: handle.clone(),
                                        role: SessionStreamRoleV1::Output,
                                    };
                                    session.insert_mapping(
                                        transport_stream_id,
                                        handle,
                                        SessionStreamRoleV1::Output,
                                    )?;
                                    session.append_mapping_once(mapping.clone()).await?;
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
                    };
                    let (response, introduced) = self
                        .producer
                        .run_owned(memory, move |_| mapping_update)
                        .await?;
                    nested_streams.extend(introduced);
                    response
                }
                CommittedProducerStreamEventPayloadV1::PackedU8(value) => {
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
                            CommittedProducerStreamEventPayloadV1::PackedU8(byte)
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
        handle: &DurableStreamHandleV1,
        through: golem_common::model::durable_stream::StreamOffsetV1,
    ) -> Result<(Vec<(u64, DurableStreamHandleV1)>, bool), String> {
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
                    .mapping_for_handle(handle, SessionStreamRoleV1::Output)
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
                        .mapping_for_handle(&nested_handle, SessionStreamRoleV1::Output)
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

    pub(crate) async fn terminal_output_cursor_stream_ids(
        &self,
        cursors: &HashMap<
            golem_common::model::durable_stream::StreamId,
            Option<golem_common::model::durable_stream::StreamOffsetV1>,
        >,
    ) -> Result<HashSet<golem_common::model::durable_stream::StreamId>, String> {
        self.recover_session_mappings().await?;
        let candidates = self
            .mappings
            .read()
            .expect("durable stream mapping lock poisoned")
            .values()
            .filter_map(|(handle, role)| {
                if *role != SessionStreamRoleV1::Output {
                    return None;
                }
                cursors
                    .get(&handle.stream_id)
                    .copied()
                    .flatten()
                    .map(|cursor| (handle.clone(), cursor))
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

    pub(crate) async fn session_root_output_mapping_ids(&self) -> Result<Vec<u64>, String> {
        Ok(self.current_control_metadata().await?.root_outputs.clone())
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
        handle: DurableStreamHandleV1,
        after: Option<golem_common::model::durable_stream::StreamOffsetV1>,
        role: SessionStreamRoleV1,
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
                        StreamSessionRecordV1::ConsumerItemValue(record) => {
                            record.session_key == self.session_key && record.stream_id == stream_id
                        }
                        StreamSessionRecordV1::ConsumerTerminal(record) => {
                            record.session_key == self.session_key && record.stream_id == stream_id
                        }
                        StreamSessionRecordV1::SourceUnavailable(record) => {
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
            VecDeque<CommittedProducerStreamEventV1>,
            Option<golem_common::model::durable_stream::StreamOffsetV1>,
            u64,
            bool,
        ),
        String,
    > {
        let mut events = Vec::new();
        for index in self.consumer_history_positions(stream_id).await? {
            let record = self.session_record_at(index).await?;
            match record {
                StreamSessionRecordV1::ConsumerItemValue(record)
                    if record.session_key == self.session_key && record.stream_id == stream_id =>
                {
                    for mapping in &record.recursive_mappings {
                        self.insert_mapping(
                            mapping.transport_stream_id,
                            mapping.handle.clone(),
                            mapping.role,
                        )?;
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
                                CommittedProducerStreamEventV1 {
                                    stream_id,
                                    producer_sequence: ordinal,
                                    offset: record.source_offset_at(index).ok_or_else(|| {
                                        "packed-u8 consumer journal offset range is invalid"
                                            .to_string()
                                    })?,
                                    packed_u8_batch_end: Some(batch_end),
                                    terminal_author: None,
                                    nested_handles: Vec::new(),
                                    payload: CommittedProducerStreamEventPayloadV1::PackedU8(byte),
                                },
                            ));
                        }
                    } else {
                        events.push((
                            record.consumer_read_ordinal,
                            CommittedProducerStreamEventV1 {
                                stream_id,
                                producer_sequence: record.consumer_read_ordinal,
                                offset: record.source_offset,
                                packed_u8_batch_end: None,
                                terminal_author: None,
                                nested_handles: record.recursive_handles,
                                payload: CommittedProducerStreamEventPayloadV1::Value(record.value),
                            },
                        ));
                    }
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
                            packed_u8_batch_end: None,
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
                            packed_u8_batch_end: None,
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
    left.attachment_id == right.attachment_id
        && left.stream_id == right.stream_id
        && left.consumer_environment_id == right.consumer_environment_id
        && left.consumer == right.consumer
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

impl DurableInputEndpoint {
    fn forwarded_handle(&self) -> Result<DurableStreamHandleV1, String> {
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
    Forwarded(DurableStreamHandleV1),
    Endpoint(DurableStreamHandleV1),
}

impl ForwardedDurableInputReference {
    fn handle(&self) -> &DurableStreamHandleV1 {
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
        source: Arc<DurableStreamProducer>,
        handle: Box<DurableStreamHandleV1>,
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
        after: Option<golem_common::model::durable_stream::StreamOffsetV1>,
    ) -> Result<usize, DurableStreamProducerError> {
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
    reader: Option<&mut DurableStreamReader>,
    after: Option<golem_common::model::durable_stream::StreamOffsetV1>,
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
    attachment: StreamAttachmentKeyV1,
    handle: DurableStreamHandleV1,
    consumer_producer: Option<Arc<DurableStreamProducer>>,
    after: Option<golem_common::model::durable_stream::StreamOffsetV1>,
    buffered: VecDeque<CommittedProducerStreamEventV1>,
    terminal: bool,
    next_journal_lag_sample: Instant,
}

impl AttachedDurableCatchUpReader {
    async fn source_unavailable_overlay(
        &self,
    ) -> Result<Option<CommittedProducerStreamEventV1>, DurableStreamProducerError> {
        let Some(producer) = &self.consumer_producer else {
            return Ok(None);
        };
        let source_offset = producer
            .consumer_source_unavailable(&self.attachment)
            .await?;
        Ok(source_offset.map(|offset| CommittedProducerStreamEventV1 {
            stream_id: self.handle.stream_id,
            producer_sequence: 0,
            offset,
            packed_u8_batch_end: None,
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
                        Option<CommittedProducerStreamEventV1>,
                        HashMap<u64, DurableInputEndpoint>,
                        bool,
                        VecDeque<CommittedProducerStreamEventV1>,
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
    drop_event_sink: Option<mpsc::UnboundedSender<DropEvent>>,
    runtime_teardown: Arc<dyn Fn() -> bool + Send + Sync>,
}

pub struct DroppedDurableInput {
    streams: DurableSessionStreams,
    transport_stream_id: u64,
    role: StreamCancelRoleV1,
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
                StreamCancelReasonV1::GuestDrop,
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
    role: StreamCancelRoleV1,
    reason: StreamCancelReasonV1,
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
            let mut queued_events = VecDeque::<CommittedProducerStreamEventV1>::new();
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
                                    CommittedProducerStreamEventPayloadV1::PackedU8(byte)
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
                        StreamSessionRecordV1::ConsumerItemValue(StreamConsumerItemValueRecordV1 {
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
            SessionStreamRoleV1::Input => StreamCancelRoleV1::InputConsumer,
            SessionStreamRoleV1::Output => StreamCancelRoleV1::OutputConsumer,
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
            CommittedProducerStreamEventPayloadV1::Value(bytes) => {
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
                role: StreamCancelRoleV1::InputProducer | StreamCancelRoleV1::OutputProducer,
                ..
            } => {
                self.finished = true;
                return Poll::Ready(Ok(StreamResult::Dropped));
            }
            CommittedProducerStreamEventPayloadV1::Cancel {
                role: StreamCancelRoleV1::InputConsumer | StreamCancelRoleV1::OutputConsumer,
                ..
            } => {
                self.finished = true;
                return Poll::Ready(Ok(StreamResult::Cancelled));
            }
            CommittedProducerStreamEventPayloadV1::Cancel {
                role: role @ StreamCancelRoleV1::System,
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

fn discards_input_after_terminal(
    error: &DurableStreamProducerError,
    session_key: &StreamSessionKeyV1,
) -> bool {
    matches!(
        error,
        DurableStreamProducerError::SessionFinished(finished) if finished == session_key
    ) || matches!(
        error,
        DurableStreamProducerError::FencedByTerminal(
            CommittedProducerStreamEventPayloadV1::Cancel {
                role: StreamCancelRoleV1::InputConsumer,
                ..
            }
        )
    )
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
        TestIdentity, TestOplog, attachment_key, identity, registration,
    };
    use crate::durable_host::stream_transport::{output_stream_pair, test_output_stream_pair};
    use crate::services::oplog::CommitLevel;
    use crate::services::rpc::{DurableStreamReadError, RpcDemand, RpcError};
    use golem_api_grpc::proto::golem::schema::{
        ListValue, SchemaValueStreamReference, schema_value,
    };
    use golem_common::base_model::component::{ComponentId, ComponentRevision};
    use golem_common::base_model::durable_stream::{
        AttachmentId, PersistedStreamInvocationDescriptorV1, ResumeAttemptDescriptorV1,
        StartAttemptDescriptorV1, StreamAttachmentKeyV1, StreamId, StreamInvocationIdV1,
        StreamOffsetV1, StreamSessionAttachedRecordV1, StreamSessionFinishedRecordV1,
        StreamSessionMappingV1, StreamSessionPreparedRecordV1,
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

    #[test]
    fn system_durable_stream_cancellation_is_a_permanent_stream_error() {
        let error = durable_stream_cancel_error(
            StreamCancelRoleV1::System,
            StreamCancelReasonV1::Cancelled,
            Some("source stopped".to_string()),
        );

        let classified = error
            .downcast_ref::<ClassifiedHostError>()
            .expect("system stream cancellation must retain its retry classification");
        assert_eq!(classified.kind, HostFailureKind::Permanent);
        assert_eq!(
            classified.message,
            "durable stream cancelled (System, Cancelled): source stopped"
        );
    }

    struct TestConsumerJournal(Arc<dyn Oplog>);

    #[async_trait::async_trait]
    impl DurableStreamConsumerJournal for TestConsumerJournal {
        async fn commit(&self) -> Result<(), String> {
            self.0.commit(CommitLevel::Always).await;
            Ok(())
        }

        async fn committed_finished_index(
            &self,
            _session: &StreamSessionKeyV1,
        ) -> Result<Option<OplogIndex>, String> {
            Ok(None)
        }
    }

    async fn append_prepared_pending(
        producer: &DurableStreamProducer,
        oplog: &TestOplog,
        identity: &TestIdentity,
        attachment_id: AttachmentId,
        attempt_id: AttemptId,
        handle: &DurableStreamHandleV1,
        role: SessionStreamRoleV1,
    ) -> OplogIndex {
        let stream_mappings = if role == SessionStreamRoleV1::Input {
            vec![StreamSessionMappingRecordV1 {
                transport_stream_id: 7,
                handle: handle.clone(),
                role,
            }]
        } else {
            Vec::new()
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
                            stream_handles: stream_mappings
                                .iter()
                                .map(|mapping| mapping.handle.clone())
                                .collect(),
                            execution_config: vec![2],
                            effective_identity: vec![3],
                        },
                        effective_identity: vec![3],
                        live_join_buffer_events: 8,
                    },
                    stream_mappings,
                },
            ))
            .await
            .unwrap();
        oplog
            .add(OplogEntry::pending_agent_invocation(
                identity.invocation.idempotency_key.clone(),
                OplogPayload::Inline(Box::new(AgentInvocationPayload::SaveSnapshot)),
                TraceId::generate(),
                Vec::new(),
                Vec::new(),
            ))
            .await
    }

    #[test]
    fn private_cancellation_mapping_preserves_durable_failure_reasons() {
        use golem_api_grpc::proto::golem::worker::StreamCancelReason;

        for (durable, proto) in [
            (
                StreamCancelReasonV1::Cancelled,
                StreamCancelReason::Cancelled,
            ),
            (
                StreamCancelReasonV1::GuestDrop,
                StreamCancelReason::ConsumerDrop,
            ),
            (StreamCancelReasonV1::Protocol, StreamCancelReason::Protocol),
            (
                StreamCancelReasonV1::InvocationFailed,
                StreamCancelReason::InvocationFailed,
            ),
            (
                StreamCancelReasonV1::SourceUnavailable,
                StreamCancelReason::SourceUnavailable,
            ),
            (
                StreamCancelReasonV1::ProducerDeleting,
                StreamCancelReason::ProducerDeleting,
            ),
        ] {
            assert_eq!(stream_cancel_reason_to_proto(durable), proto);
        }
    }

    #[test]
    async fn guest_owned_u8_output_uses_the_packed_durable_path() {
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
                    root_kind: StreamRootKindV1::MethodResult,
                    recursive_value_path: Vec::new(),
                },
                StreamSourceKindV1::InvocationOutput,
            ))
            .await
            .unwrap()
            .value;
        let entries_before_output = oplog.length().await;
        let streams =
            DurableSessionStreams::new(producer.clone(), oplog.clone(), identity.invocation, []);
        let (publisher, endpoint) = test_output_stream_pair(4).unwrap();
        let (nested_tx, _nested_rx) = mpsc::unbounded_channel();
        let drain = PendingOwnedStreamDrain {
            handle: handle.clone(),
            endpoint,
            element_type: SchemaType::u8(),
            role: SessionStreamRoleV1::Output,
        };
        let drain_task = tokio::spawn(async move {
            streams
                .drain_output(
                    drain,
                    Arc::new(SchemaGraph::anonymous(SchemaType::u8())),
                    nested_tx,
                )
                .await
        });

        publisher.publish_item(SchemaValue::U8(10)).await.unwrap();
        publisher.publish_item(SchemaValue::U8(11)).await.unwrap();
        publisher.publish_end().await.unwrap();
        drain_task.await.unwrap().unwrap();
        assert_eq!(
            oplog.length().await,
            entries_before_output + 2,
            "consecutive guest u8 values must share one durable item batch"
        );

        let mut reader = producer.catch_up(handle, None).await.unwrap();
        assert!(matches!(
            reader.next().await.unwrap().unwrap().payload,
            CommittedProducerStreamEventPayloadV1::PackedU8(10)
        ));
        assert!(matches!(
            reader.next().await.unwrap().unwrap().payload,
            CommittedProducerStreamEventPayloadV1::PackedU8(11)
        ));
        assert!(matches!(
            reader.next().await.unwrap().unwrap().payload,
            CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::Ok)
        ));
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn materialized_output_releases_admission_and_survives_abandoned_response() {
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
        let streams = DurableSessionStreams::new(producer.clone(), oplog, identity.invocation, []);
        let (publisher, endpoint) = test_output_stream_pair(4).unwrap();
        let task_streams = streams.clone();
        let materialization = tokio::spawn(async move {
            let root = SchemaType::stream(Some(SchemaType::u8()));
            task_streams
                .materialize_result(
                    SchemaValue::Stream(SchemaValueStream::from_host_endpoint(endpoint)),
                    &SchemaGraph::anonymous(root.clone()),
                    &root,
                    ComponentRevision::INITIAL,
                )
                .await
        });
        streams.wait_persisted_result().await.unwrap();
        let handle = streams
            .remote_result_record()
            .await
            .unwrap()
            .unwrap()
            .output_streams[0]
            .clone();

        // The live drain must leave all sixteen normal operation slots available.
        let admitted = Arc::new(tokio::sync::Barrier::new(17));
        let mut operations = tokio::task::JoinSet::new();
        for _ in 0..16 {
            let producer = producer.clone();
            let admitted = admitted.clone();
            operations.spawn(async move {
                producer
                    .run_owned(0, move |_| async move {
                        admitted.wait().await;
                        Ok::<_, String>(())
                    })
                    .await
            });
        }
        tokio::time::timeout(std::time::Duration::from_secs(5), admitted.wait())
            .await
            .expect("a long-lived output drain retained mutation admission");
        while let Some(result) = operations.join_next().await {
            result.unwrap().unwrap();
        }
        materialization.abort();
        assert!(materialization.await.unwrap_err().is_cancelled());

        publisher.publish_item(SchemaValue::U8(37)).await.unwrap();
        publisher.publish_item(SchemaValue::U8(81)).await.unwrap();
        publisher.publish_end().await.unwrap();
        let mut reader = producer.catch_up(handle, None).await.unwrap();
        for expected in [37, 81] {
            assert_eq!(
                reader.next().await.unwrap().unwrap().payload,
                CommittedProducerStreamEventPayloadV1::PackedU8(expected)
            );
        }
        assert_eq!(
            reader.next().await.unwrap().unwrap().payload,
            CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::Ok)
        );
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn concurrent_nested_mapping_reuses_identity_before_commit_callback_finishes() {
        use crate::durable_host::durable_stream::DurableStreamCommit;
        use std::sync::atomic::AtomicBool;

        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let block = Arc::new(AtomicBool::new(false));
        let reached = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let commit: DurableStreamCommit = Arc::new({
            let oplog = oplog.clone();
            let block = block.clone();
            let reached = reached.clone();
            let release = release.clone();
            move |receipt| {
                let oplog = oplog.clone();
                let block = block.clone();
                let reached = reached.clone();
                let release = release.clone();
                Box::pin(async move {
                    oplog.commit(CommitLevel::Always).await;
                    if let Some(receipt) = receipt {
                        let _ = receipt.send(());
                    }
                    if block.load(Ordering::Acquire) {
                        reached.notify_one();
                        release.acquire().await.unwrap().forget();
                    }
                })
            }
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
        let streams = DurableSessionStreams::new(producer, oplog, identity.invocation, []);
        block.store(true, Ordering::Release);
        let first = tokio::spawn({
            let streams = streams.clone();
            let handle = handle.clone();
            async move {
                streams
                    .ensure_nested_mapping(handle, SessionStreamRoleV1::Output)
                    .await
            }
        });
        reached.notified().await;
        let mut second = tokio::spawn({
            let streams = streams.clone();
            let handle = handle.clone();
            async move {
                streams
                    .ensure_nested_mapping(handle, SessionStreamRoleV1::Output)
                    .await
            }
        });
        let second_result = tokio::select! {
            result = &mut second => Some(result),
            _ = reached.notified() => None,
        };
        release.add_permits(2);
        let first = first.await.unwrap().unwrap();
        let second = match second_result {
            Some(result) => result,
            None => second.await,
        }
        .unwrap()
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(first.handle, handle);
        assert_eq!(
            streams
                .current_control_metadata()
                .await
                .unwrap()
                .persisted_mappings
                .len(),
            1
        );
    }

    async fn activate_test_root_attachment(
        producer: &DurableStreamProducer,
        identity: &TestIdentity,
        handle: &DurableStreamHandleV1,
    ) {
        let mut consumer_invocation = identity.invocation.clone();
        consumer_invocation.callee.agent_id = "distinct-root-consumer".to_string();
        let attachment = StreamAttachmentKeyV1 {
            attachment_id: AttachmentId::primary(
                identity.environment_id,
                &identity.agent_id,
                &identity.invocation.idempotency_key,
            )
            .unwrap(),
            stream_id: handle.stream_id,
            epoch: 1,
            session_key: identity.invocation.clone(),
            producer_environment_id: handle.producer_environment_id,
            producer: handle.producer.clone(),
            expected_producer_fingerprint: handle.expected_producer_fingerprint,
            consumer_environment_id: consumer_invocation.callee_environment_id,
            consumer: consumer_invocation.callee.clone(),
            expected_consumer_fingerprint: consumer_invocation.callee_fingerprint,
            consumer_invocation,
        };
        let now = Timestamp::now_utc().to_millis();
        producer
            .prepare_attachment(attachment.clone(), now)
            .await
            .unwrap();
        producer.activate_attachment(attachment, now).await.unwrap();
    }

    async fn assert_local_nested_stream_drains_after_root_admission(root_kind: StreamRootKindV1) {
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
        .with_consumer_journal(Arc::new(TestConsumerJournal(oplog)))
        .require_root_attachment_before_production();
        let child_type = SchemaType::stream(Some(SchemaType::u8()));
        let root_type = SchemaType::stream(Some(child_type));
        let graph = SchemaGraph::anonymous(root_type.clone());
        let (root_publisher, root_endpoint) = test_output_stream_pair(4).unwrap();

        let materialize_result = match root_kind {
            StreamRootKindV1::MethodInput => {
                streams
                    .materialize_agent_input(
                        &SchemaValue::Stream(SchemaValueStream::from_host_endpoint(root_endpoint)),
                        &graph,
                        &root_type,
                        ComponentRevision::INITIAL,
                    )
                    .await
                    .unwrap();
                None
            }
            StreamRootKindV1::MethodResult => {
                let streams = streams.clone();
                let graph = graph.clone();
                let root_type = root_type.clone();
                Some(tokio::spawn(async move {
                    streams
                        .materialize_result(
                            SchemaValue::Stream(SchemaValueStream::from_host_endpoint(
                                root_endpoint,
                            )),
                            &graph,
                            &root_type,
                            ComponentRevision::INITIAL,
                        )
                        .await
                }))
            }
        };
        let coordinate = StreamRegistrationCoordinateV1::Root {
            invocation_id: identity.invocation.clone(),
            root_kind,
            recursive_value_path: Vec::new(),
        };
        let root_handle = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if let Some(handle) = producer.handle_for_coordinate(&coordinate).await.unwrap() {
                    break handle;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("root stream was not registered");
        assert_eq!(
            producer
                .input_high_water(root_handle.stream_id)
                .await
                .unwrap(),
            None
        );

        activate_test_root_attachment(&producer, &identity, &root_handle).await;
        let (first_publisher, first_endpoint) = test_output_stream_pair(2).unwrap();
        let (second_publisher, second_endpoint) = test_output_stream_pair(2).unwrap();
        root_publisher
            .publish_item(SchemaValue::Stream(SchemaValueStream::from_host_endpoint(
                first_endpoint,
            )))
            .await
            .unwrap();
        root_publisher
            .publish_item(SchemaValue::Stream(SchemaValueStream::from_host_endpoint(
                second_endpoint,
            )))
            .await
            .unwrap();
        root_publisher.publish_end().await.unwrap();
        wait_for_terminal_commit(&producer, root_handle.stream_id).await;

        // Both children are queued before the parent's final join, but remain live afterwards.
        let first_handle = producer
            .nested_handles(root_handle.stream_id, 0)
            .await
            .unwrap()[0]
            .clone();
        let second_handle = producer
            .nested_handles(root_handle.stream_id, 1)
            .await
            .unwrap()[0]
            .clone();
        first_publisher
            .publish_item(SchemaValue::U8(17))
            .await
            .unwrap();
        first_publisher.publish_end().await.unwrap();
        second_publisher
            .publish_item(SchemaValue::U8(29))
            .await
            .unwrap();
        second_publisher.publish_end().await.unwrap();

        for (handle, expected) in [(first_handle, 17), (second_handle, 29)] {
            let mut reader = DurableStreamReader::Owned {
                reader: Box::new(producer.catch_up(handle.clone(), None).await.unwrap()),
                source: producer.clone(),
                handle: Box::new(handle),
                next_journal_lag_sample: Instant::now(),
            };
            assert!(matches!(
                tokio::time::timeout(Duration::from_secs(1), reader.next()).await.unwrap().unwrap().unwrap().payload,
                CommittedProducerStreamEventPayloadV1::PackedU8(value) if value == expected
            ));
            assert!(matches!(
                reader.next().await.unwrap().unwrap().payload,
                CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::Ok)
            ));
        }
        assert_eq!(
            StreamAttachmentControl::inspect_attachments(producer.as_ref())
                .await
                .len(),
            1,
            "local nested streams must not require independent attachments"
        );
        if let Some(task) = materialize_result {
            task.await.unwrap().unwrap();
        }
    }

    #[test]
    async fn caller_input_local_nested_streams_inherit_root_admission() {
        assert_local_nested_stream_drains_after_root_admission(StreamRootKindV1::MethodInput).await;
    }

    #[test]
    async fn result_local_nested_streams_inherit_root_admission() {
        assert_local_nested_stream_drains_after_root_admission(StreamRootKindV1::MethodResult)
            .await;
    }

    struct RecordingConsumerJournal {
        oplog: Arc<dyn Oplog>,
        commits: Arc<AtomicU64>,
    }

    type LagSample = (Option<StreamOffsetV1>, Result<usize, ()>);

    struct LagRecordingSource {
        producer: Arc<DurableStreamProducer>,
        calls: Mutex<Vec<LagSample>>,
        failures_remaining: AtomicU64,
    }

    impl LagRecordingSource {
        fn new(producer: Arc<DurableStreamProducer>, failures: u64) -> Arc<Self> {
            Arc::new(Self {
                producer,
                calls: Mutex::new(Vec::new()),
                failures_remaining: AtomicU64::new(failures),
            })
        }
    }

    #[async_trait::async_trait]
    impl AttachedStreamSegmentSource for LagRecordingSource {
        async fn journal_lag_events(
            &self,
            handle: &DurableStreamHandleV1,
            after: Option<StreamOffsetV1>,
        ) -> Result<usize, DurableStreamProducerError> {
            if self
                .failures_remaining
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
            {
                self.calls.lock().await.push((after, Err(())));
                return Err(DurableStreamProducerError::InvalidHandle);
            }
            let lag = self.producer.journal_lag_events(handle, after).await?;
            self.calls.lock().await.push((after, Ok(lag)));
            Ok(lag)
        }

        async fn read_attached_segment(
            &self,
            attachment: &StreamAttachmentKeyV1,
            handle: &DurableStreamHandleV1,
            now_millis: u64,
            after: Option<StreamOffsetV1>,
            through: Option<StreamOffsetV1>,
        ) -> Result<Vec<CommittedProducerStreamEventV1>, DurableStreamProducerError> {
            self.producer
                .read_attached_segment(attachment, handle, now_millis, after, through)
                .await
        }

        async fn wait_for_attached_segment(
            &self,
            attachment: &StreamAttachmentKeyV1,
            handle: &DurableStreamHandleV1,
            now_millis: u64,
            after: Option<StreamOffsetV1>,
        ) -> Result<Vec<CommittedProducerStreamEventV1>, DurableStreamProducerError> {
            self.producer
                .wait_for_attached_segment(attachment, handle, now_millis, after)
                .await
        }
    }

    struct AttachedProducerRpc {
        producer: Arc<DurableStreamProducer>,
        cancellation_owner: Option<Arc<DurableStreamProducer>>,
        scripted_reads: Mutex<VecDeque<Result<Vec<u8>, DurableStreamReadError<RpcError>>>>,
        pending_read: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
        read_requests:
            Mutex<Vec<golem_common::base_model::durable_stream::AttachedStreamSegmentRequestV1>>,
    }

    #[async_trait::async_trait]
    impl Rpc for AttachedProducerRpc {
        async fn control_durable_stream_attachment(
            &self,
            request: golem_common::base_model::durable_stream::StreamAttachmentControlRequestV1,
            _auth_ctx: &AuthCtx,
        ) -> Result<bool, RpcError> {
            let owner = self
                .cancellation_owner
                .as_ref()
                .expect("unexpected control RPC");
            assert!(matches!(
                request.operation,
                golem_common::base_model::durable_stream::StreamAttachmentControlOperationV1::Cancel {
                    role: StreamCancelRoleV1::InputConsumer,
                    reason: StreamCancelReasonV1::GuestDrop,
                    ..
                } | golem_common::base_model::durable_stream::StreamAttachmentControlOperationV1::Prepare { .. }
            ));
            owner.poison();
            owner.wait_durable_drained().await;
            Ok(true)
        }

        async fn create_demand(
            &self,
            _owned_agent_id: &OwnedAgentId,
            _method_name: &str,
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
            _scope_card: Option<golem_common::base_model::card::ScopeCard>,
        ) -> Result<SchemaValue, RpcError> {
            unreachable!("test RPC only serves attached stream segments")
        }

        async fn read_durable_stream_segment(
            &self,
            request: golem_common::base_model::durable_stream::DurableStreamReadRequestV1,
            _auth_ctx: &AuthCtx,
        ) -> Result<Vec<u8>, DurableStreamReadError<RpcError>> {
            let golem_common::base_model::durable_stream::DurableStreamReadRequestV1::AttachedConsumer(request) = request else {
                unreachable!("test RPC only serves attached stream segments")
            };
            self.read_requests.lock().await.push(*request.clone());
            if let Some(pending) = self.pending_read.lock().await.take() {
                pending.await.unwrap();
            }
            if let Some(response) = self.scripted_reads.lock().await.pop_front() {
                return response;
            }
            let events = if request.wait_for_events {
                self.producer
                    .wait_for_attached_segment(
                        &request.attachment,
                        &request.mapping.handle,
                        113,
                        request.after,
                    )
                    .await
            } else {
                self.producer
                    .read_attached_segment(
                        &request.attachment,
                        &request.mapping.handle,
                        113,
                        request.after,
                        request.through,
                    )
                    .await
            }
            .map_err(|error| {
                DurableStreamReadError::from_producer(error, |details| RpcError::ProtocolError {
                    details,
                })
            })?;
            golem_common::serialization::serialize(&events)
                .map_err(|error| RpcError::ProtocolError {
                    details: error.to_string(),
                })
                .map_err(Into::into)
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

    #[test]
    #[test_r::timeout("10s")]
    async fn routed_attached_reads_retry_only_unavailable_without_changing_the_cursor() {
        let identity = identity();
        let producer = DurableStreamProducer::load(
            Arc::new(TestOplog::default()),
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
        let attachment = attachment_key(&identity, handle.stream_id);
        let mapping = StreamSessionMappingRecordV1 {
            transport_stream_id: 17,
            handle: handle.clone(),
            role: SessionStreamRoleV1::Input,
        };
        let rpc = Arc::new(AttachedProducerRpc {
            producer: producer.clone(),
            cancellation_owner: None,
            scripted_reads: Mutex::default(),
            pending_read: Mutex::default(),
            read_requests: Mutex::default(),
        });
        let source = RoutedAttachedStreamSegmentSource::new(
            rpc.clone(),
            mapping.clone(),
            AuthCtx::System,
            producer,
        );
        let after = Some(StreamOffsetV1::new(OplogIndex::from_u64(41), 3));
        let through = Some(StreamOffsetV1::new(OplogIndex::from_u64(59), 7));
        let expected = vec![CommittedProducerStreamEventV1 {
            stream_id: handle.stream_id,
            producer_sequence: 11,
            offset: StreamOffsetV1::new(OplogIndex::from_u64(43), 4),
            packed_u8_batch_end: None,
            terminal_author: None,
            nested_handles: Vec::new(),
            payload: CommittedProducerStreamEventPayloadV1::PackedU8(37),
        }];
        for wait_for_events in [false, true] {
            let read = || async {
                if wait_for_events {
                    source
                        .wait_for_attached_segment(&attachment, &handle, 113, after)
                        .await
                } else {
                    source
                        .read_attached_segment(&attachment, &handle, 113, after, through)
                        .await
                }
            };
            rpc.scripted_reads.lock().await.extend([
                Err(DurableStreamReadError::Unavailable),
                Err(DurableStreamReadError::Unavailable),
                Ok(golem_common::serialization::serialize(&expected).unwrap()),
            ]);
            assert_eq!(read().await.unwrap(), expected);
            let requests = std::mem::take(&mut *rpc.read_requests.lock().await);
            assert_eq!(requests.len(), 3);
            for request in requests {
                assert_eq!(request.attachment, attachment);
                assert_eq!(request.mapping, mapping);
                assert_eq!(request.after, after);
                assert_eq!(
                    request.through,
                    if wait_for_events { None } else { through }
                );
                assert_eq!(request.wait_for_events, wait_for_events);
            }

            rpc.scripted_reads
                .lock()
                .await
                .push_back(Err(DurableStreamReadError::Other(RpcError::Denied {
                    details: "access revoked".to_string(),
                })));
            assert!(
                matches!(read().await, Err(DurableStreamProducerError::Oplog(message)) if message.contains("access revoked"))
            );
            assert_eq!(rpc.read_requests.lock().await.len(), 1);
            rpc.read_requests.lock().await.clear();

            rpc.scripted_reads
                .lock()
                .await
                .push_back(Err(DurableStreamReadError::Unavailable));
            let mut pending = Box::pin(read());
            assert!(futures::poll!(pending.as_mut()).is_pending());
            assert_eq!(rpc.read_requests.lock().await.len(), 1);
            drop(pending);
            tokio::time::sleep(Duration::from_millis(150)).await;
            assert_eq!(rpc.read_requests.lock().await.len(), 1);
            rpc.read_requests.lock().await.clear();

            let (sender, receiver) = tokio::sync::oneshot::channel();
            *rpc.pending_read.lock().await = Some(receiver);
            let mut pending = Box::pin(read());
            assert!(futures::poll!(pending.as_mut()).is_pending());
            assert!(!sender.is_closed());
            assert_eq!(rpc.read_requests.lock().await.len(), 1);
            drop(pending);
            assert!(sender.is_closed());
            rpc.read_requests.lock().await.clear();
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
    fn late_input_is_discarded_after_the_session_or_consumer_terminates() {
        let session_key = identity().invocation;
        assert!(discards_input_after_terminal(
            &DurableStreamProducerError::SessionFinished(session_key.clone()),
            &session_key,
        ));
        assert!(discards_input_after_terminal(
            &DurableStreamProducerError::FencedByTerminal(
                CommittedProducerStreamEventPayloadV1::Cancel {
                    role: StreamCancelRoleV1::InputConsumer,
                    reason: StreamCancelReasonV1::GuestDrop,
                    details: None,
                },
            ),
            &session_key,
        ));
        assert!(!discards_input_after_terminal(
            &DurableStreamProducerError::FencedByTerminal(
                CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::Ok),
            ),
            &session_key,
        ));
    }

    async fn backpressured_session_input() -> (
        Arc<DurableStreamProducer>,
        DurableSessionStreams,
        DurableCatchUpReader,
        DurableStreamHandleV1,
    ) {
        let identity = identity();
        let oplog = Arc::new(TestOplog::default());
        let producer = DurableStreamProducer::load(
            oplog.clone(),
            identity.environment_id,
            identity.agent_id.clone(),
            identity.fingerprint,
            Some(1),
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
        let pending_invocation_oplog_index = append_prepared_pending(
            producer.as_ref(),
            oplog.as_ref(),
            &identity,
            attachment_id,
            attempt_id,
            &handle,
            SessionStreamRoleV1::Input,
        )
        .await;
        producer
            .append_session_record(StreamSessionRecordV1::Attached(
                StreamSessionAttachedRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: identity.invocation.clone(),
                    attachment_id,
                    attempt_id,
                    epoch: 1,
                    pending_invocation_oplog_index,
                },
            ))
            .await
            .unwrap();
        let streams = DurableSessionStreams::new(
            producer.clone(),
            oplog.clone(),
            identity.invocation,
            [(7, handle.clone(), SessionStreamRoleV1::Input)],
        )
        .with_consumer_journal(Arc::new(TestConsumerJournal(oplog)));
        let reader = producer.catch_up(handle.clone(), None).await.unwrap();
        producer
            .write_items(handle.stream_id, 0, StreamItemsPayloadV1::PackedU8(vec![1]))
            .await
            .unwrap();
        (producer, streams, reader, handle)
    }

    async fn wait_for_terminal_commit(producer: &DurableStreamProducer, stream_id: StreamId) {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if producer
                    .input_high_water(stream_id)
                    .await
                    .unwrap()
                    .is_some_and(|high_water| high_water.terminal)
                {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("stream terminal was not committed");
    }

    #[test]
    #[test_r::timeout("15s")]
    async fn foreign_cancellation_can_drain_its_local_owner_from_the_rpc_callback() {
        retirement_from_foreign_rpc(false).await;
    }

    #[test]
    #[test_r::timeout("15s")]
    async fn foreign_preparation_releases_local_owner_when_rpc_requests_retirement() {
        retirement_from_foreign_rpc(true).await;
    }

    async fn retirement_from_foreign_rpc(prepare: bool) {
        let local = identity();
        let mut remote = identity();
        remote.agent_id.agent_id.push_str("-remote");
        remote.invocation.callee = remote.agent_id.clone();
        let remote_producer = DurableStreamProducer::load(
            Arc::new(TestOplog::default()),
            remote.environment_id,
            remote.agent_id.clone(),
            remote.fingerprint,
            None,
        )
        .await
        .unwrap();
        let handle = remote_producer
            .register(registration(
                &remote,
                StreamRegistrationCoordinateV1::Root {
                    invocation_id: remote.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodResult,
                    recursive_value_path: Vec::new(),
                },
                StreamSourceKindV1::InvocationOutput,
            ))
            .await
            .unwrap()
            .value;
        let oplog = Arc::new(TestOplog::default());
        let producer = DurableStreamProducer::load(
            oplog.clone(),
            local.environment_id,
            local.agent_id.clone(),
            local.fingerprint,
            None,
        )
        .await
        .unwrap();
        let attachment_id = AttachmentId::primary(
            local.environment_id,
            &local.agent_id,
            &local.invocation.idempotency_key,
        )
        .unwrap();
        let attempt_id = AttemptId::fresh();
        let pending_invocation_oplog_index = append_prepared_pending(
            &producer,
            &oplog,
            &local,
            attachment_id,
            attempt_id,
            &handle,
            SessionStreamRoleV1::Input,
        )
        .await;
        producer
            .append_session_record(StreamSessionRecordV1::Attached(
                StreamSessionAttachedRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: local.invocation.clone(),
                    attachment_id,
                    attempt_id,
                    epoch: 1,
                    pending_invocation_oplog_index,
                },
            ))
            .await
            .unwrap();
        let streams = DurableSessionStreams::new(
            producer.clone(),
            oplog.clone(),
            local.invocation.clone(),
            [(7, handle.clone(), SessionStreamRoleV1::Input)],
        )
        .with_consumer_journal(Arc::new(TestConsumerJournal(oplog.clone())))
        .with_attachment(1, attempt_id)
        .with_rpc(Arc::new(AttachedProducerRpc {
            producer: remote_producer,
            cancellation_owner: Some(producer.clone()),
            scripted_reads: Mutex::default(),
            pending_read: Mutex::default(),
            read_requests: Mutex::default(),
        }))
        .with_auth_ctx(AuthCtx::System);
        if prepare {
            let error = tokio::time::timeout(
                Duration::from_secs(5),
                streams.prepare_foreign_mapping(
                    StreamSessionMappingRecordV1 {
                        transport_stream_id: 7,
                        handle,
                        role: SessionStreamRoleV1::Input,
                    },
                    1,
                ),
            )
            .await
            .expect("remote preparation prevented local retirement")
            .unwrap_err();
            assert_eq!(
                error,
                DurableStreamProducerError::RecoveryRequired.to_string()
            );
            producer.wait_durable_drained().await;
            assert!(streams.session_lock.try_lock().is_ok());
            return;
        }
        tokio::time::timeout(
            Duration::from_secs(5),
            streams.cancel_stream(
                7,
                StreamCancelRoleV1::InputConsumer,
                StreamCancelReasonV1::GuestDrop,
                Some("reader dropped".into()),
                None,
            ),
        )
        .await
        .expect("RPC callback could not drain local producer")
        .unwrap();
        assert!(matches!(
            producer.ensure_healthy(),
            Err(DurableStreamProducerError::RecoveryRequired)
        ));
        let restarted = DurableStreamProducer::load(
            oplog.clone(),
            local.environment_id,
            local.agent_id,
            local.fingerprint,
            None,
        )
        .await
        .unwrap();
        let recovered = DurableSessionStreams::new(
            restarted,
            oplog,
            local.invocation,
            [(7, handle.clone(), SessionStreamRoleV1::Input)],
        );
        let metadata = recovered.current_control_metadata().await.unwrap();
        let intent = metadata.cancel_intents.get(&handle.stream_id).unwrap();
        assert_eq!(intent.epoch, 1);
        assert_eq!(intent.role, StreamCancelRoleV1::InputConsumer);
        assert_eq!(intent.reason, StreamCancelReasonV1::GuestDrop);
        assert_eq!(intent.details.as_deref(), Some("reader dropped"));
        assert!(streams.session_lock.try_lock().is_ok());
    }

    #[test]
    async fn local_cancellation_releases_session_lock_after_commit_before_live_publication() {
        let (producer, streams, mut reader, handle) = backpressured_session_input().await;
        let cancellation = tokio::spawn({
            let streams = streams.clone();
            async move {
                streams
                    .cancel_stream(
                        7,
                        StreamCancelRoleV1::InputConsumer,
                        StreamCancelReasonV1::GuestDrop,
                        Some("guest dropped its durable readable stream end".to_string()),
                        None,
                    )
                    .await
            }
        });

        wait_for_terminal_commit(&producer, handle.stream_id).await;
        assert!(!cancellation.is_finished());
        let session_guard = tokio::time::timeout(
            Duration::from_secs(1),
            streams.session_lock.lock(),
        )
        .await
        .expect(
            "local cancellation must release session ownership after its terminal is committed",
        );
        drop(session_guard);

        assert_eq!(reader.next().await.unwrap().unwrap().producer_sequence, 0);
        cancellation.await.unwrap().unwrap();
        let terminal = reader.next().await.unwrap().unwrap();
        assert_eq!(terminal.producer_sequence, 1);
        assert!(matches!(
            terminal.payload,
            CommittedProducerStreamEventPayloadV1::Cancel {
                role: StreamCancelRoleV1::InputConsumer,
                reason: StreamCancelReasonV1::GuestDrop,
                ..
            }
        ));
        assert!(reader.next().await.unwrap().is_none());
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
            .unwrap()
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
            .unwrap()
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
            reloaded
                .handle_for_coordinate(&input_coordinate)
                .await
                .unwrap(),
            Some(input_handle)
        );
        assert_eq!(
            reloaded
                .handle_for_coordinate(&result_coordinate)
                .await
                .unwrap(),
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

        async fn committed_finished_index(
            &self,
            _session: &StreamSessionKeyV1,
        ) -> Result<Option<OplogIndex>, String> {
            Ok(None)
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
            reader: Box::new(producer.catch_up(handle.clone(), None).await.unwrap()),
            source: producer.clone(),
            handle: Box::new(handle.clone()),
            next_journal_lag_sample: Instant::now(),
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
        let mut reader = DurableStreamReader::Attached(Box::new(AttachedDurableCatchUpReader {
            source: producer,
            attachment,
            handle,
            consumer_producer: None,
            after: None,
            buffered: VecDeque::new(),
            terminal: false,
            next_journal_lag_sample: Instant::now(),
        }));

        assert_eq!(reader.journal_lag_events(None).await.unwrap(), 3);
        let first = reader.next().await.unwrap().unwrap();
        assert_eq!(
            reader.journal_lag_events(Some(first.offset)).await.unwrap(),
            2
        );
    }

    async fn use_attached_lag_spy(
        consumer: &mut DurableInputProducer,
        producer: Arc<DurableStreamProducer>,
        identity: &TestIdentity,
        handle: &DurableStreamHandleV1,
        source: Arc<LagRecordingSource>,
    ) {
        let attachment = StreamAttachmentKeyV1 {
            attachment_id: AttachmentId::primary(
                identity.environment_id,
                &identity.agent_id,
                &identity.invocation.idempotency_key,
            )
            .unwrap(),
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
        let now_millis = Timestamp::now_utc().to_millis();
        producer
            .prepare_attachment(attachment.clone(), now_millis)
            .await
            .unwrap();
        producer
            .activate_attachment(attachment.clone(), now_millis)
            .await
            .unwrap();
        consumer.reader = Some(DurableStreamReader::Attached(Box::new(
            AttachedDurableCatchUpReader {
                source,
                attachment,
                handle: handle.clone(),
                consumer_producer: None,
                after: None,
                buffered: VecDeque::new(),
                terminal: false,
                next_journal_lag_sample: Instant::now(),
            },
        )));
    }

    async fn receive_for_test(
        consumer: &mut DurableInputProducer,
    ) -> (CommittedProducerStreamEventV1, bool, usize) {
        consumer.begin_receive();
        let (reader, event, _, journaled, queued) = consumer.pending.take().unwrap().await.unwrap();
        consumer.reader = reader;
        let queued_len = queued.len();
        consumer.journal.extend(queued);
        consumer.consumer_read_ordinal += 1;
        (event.unwrap(), journaled, queued_len)
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
        let (_, event, _, journaled, _) = first.pending.take().unwrap().await.unwrap();
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
        let (_, event, _, journaled, _) = replay.pending.take().unwrap().await.unwrap();
        assert!(journaled);
        assert!(event.is_some());
        assert_eq!(commits.load(Ordering::Relaxed), 1);
    }

    #[test]
    async fn consumer_journal_lag_sampling_is_deadline_gated_and_failure_is_throttled() {
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
        for value in 0..8 {
            producer
                .write_items(
                    handle.stream_id,
                    u64::from(value),
                    StreamItemsPayloadV1::Values(vec![
                        ProtoSchemaValue::try_from(SchemaValue::U32(value))
                            .unwrap()
                            .encode_to_vec(),
                    ]),
                )
                .await
                .unwrap();
        }
        let commits = Arc::new(AtomicU64::new(0));
        let streams = DurableSessionStreams::new(
            producer.clone(),
            oplog.clone(),
            identity.invocation.clone(),
            [(1, handle.clone(), SessionStreamRoleV1::Input)],
        )
        .with_consumer_journal(Arc::new(RecordingConsumerJournal {
            oplog,
            commits: commits.clone(),
        }));
        let mut consumer = DurableInputProducer::new(
            streams
                .endpoint(handle.clone(), 0, SessionStreamRoleV1::Input)
                .await
                .unwrap(),
        );
        let source = LagRecordingSource::new(producer.clone(), 0);
        use_attached_lag_spy(&mut consumer, producer, &identity, &handle, source.clone()).await;
        *consumer
            .reader
            .as_mut()
            .unwrap()
            .journal_lag_sample_deadline() = Instant::now() + Duration::from_secs(60);

        for expected_commits in 1..=5 {
            let (_, journaled, _) = receive_for_test(&mut consumer).await;
            assert!(!journaled);
            assert_eq!(commits.load(Ordering::Relaxed), expected_commits);
        }
        assert!(source.calls.lock().await.is_empty());

        *consumer
            .reader
            .as_mut()
            .unwrap()
            .journal_lag_sample_deadline() = Instant::now() - Duration::from_millis(1);
        let (sampled, journaled, _) = receive_for_test(&mut consumer).await;
        assert!(!journaled);
        assert_eq!(commits.load(Ordering::Relaxed), 6);
        assert_eq!(
            source.calls.lock().await.as_slice(),
            &[(Some(sampled.offset), Ok(2))]
        );

        source.failures_remaining.store(1, Ordering::Relaxed);
        let before_attempt = Instant::now();
        *consumer
            .reader
            .as_mut()
            .unwrap()
            .journal_lag_sample_deadline() = Instant::now() - Duration::from_millis(1);
        let (_, journaled, _) = receive_for_test(&mut consumer).await;
        assert!(!journaled);
        assert_eq!(commits.load(Ordering::Relaxed), 7);
        assert_eq!(source.calls.lock().await.len(), 2);
        assert!(source.calls.lock().await[1].1.is_err());
        assert!(
            before_attempt
                < *consumer
                    .reader
                    .as_mut()
                    .unwrap()
                    .journal_lag_sample_deadline()
        );
        *consumer
            .reader
            .as_mut()
            .unwrap()
            .journal_lag_sample_deadline() = Instant::now() + Duration::from_secs(60);
        let (_, journaled, _) = receive_for_test(&mut consumer).await;
        assert!(!journaled);
        assert_eq!(commits.load(Ordering::Relaxed), 8);
        assert_eq!(source.calls.lock().await.len(), 2);
    }

    #[test]
    async fn packed_consumer_samples_after_final_queued_offset_and_terminal_forces_sampling() {
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
        let bytes = (0..4096).map(|value| value as u8).collect::<Vec<_>>();
        producer
            .write_items(
                handle.stream_id,
                0,
                StreamItemsPayloadV1::PackedU8(bytes.clone()),
            )
            .await
            .unwrap();
        producer
            .end(handle.stream_id, bytes.len() as u64, StreamEndResultV1::Ok)
            .await
            .unwrap();

        let commits = Arc::new(AtomicU64::new(0));
        let streams = DurableSessionStreams::new(
            producer.clone(),
            oplog.clone(),
            identity.invocation.clone(),
            [(1, handle.clone(), SessionStreamRoleV1::Input)],
        )
        .with_consumer_journal(Arc::new(RecordingConsumerJournal {
            oplog,
            commits: commits.clone(),
        }));
        let mut consumer = DurableInputProducer::new(
            streams
                .endpoint(handle.clone(), 0, SessionStreamRoleV1::Input)
                .await
                .unwrap(),
        );
        let source = LagRecordingSource::new(producer.clone(), 0);
        use_attached_lag_spy(&mut consumer, producer, &identity, &handle, source.clone()).await;
        *consumer
            .reader
            .as_mut()
            .unwrap()
            .journal_lag_sample_deadline() = Instant::now() - Duration::from_millis(1);

        let (first, journaled, queued) = receive_for_test(&mut consumer).await;
        assert!(!journaled);
        assert_eq!(queued, bytes.len() - 1);
        assert_eq!(commits.load(Ordering::Relaxed), 1);
        {
            let calls = source.calls.lock().await;
            assert_eq!(calls.len(), 1);
            let (sampled_after, lag) = calls[0];
            assert_eq!(
                sampled_after,
                Some(StreamOffsetV1::new(
                    first.offset.producer_oplog_index(),
                    bytes.len() as u32 - 1
                ))
            );
            assert_eq!(lag, Ok(1));
        }

        for _ in 1..bytes.len() {
            let (_, journaled, _) = receive_for_test(&mut consumer).await;
            assert!(journaled);
        }
        assert_eq!(commits.load(Ordering::Relaxed), 1);
        assert_eq!(source.calls.lock().await.len(), 1);

        *consumer
            .reader
            .as_mut()
            .unwrap()
            .journal_lag_sample_deadline() = Instant::now() + Duration::from_secs(60);
        let (terminal, journaled, _) = receive_for_test(&mut consumer).await;
        assert!(terminal.is_terminal());
        assert!(!journaled);
        assert_eq!(commits.load(Ordering::Relaxed), 2);
        let calls = source.calls.lock().await;
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1], (Some(terminal.offset), Ok(0)));
    }

    #[test]
    async fn packed_u8_consumer_values_share_one_durable_journal_record() {
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
        let bytes = (0..2048).map(|value| value as u8).collect::<Vec<_>>();

        let commits = Arc::new(AtomicU64::new(0));
        let streams = DurableSessionStreams::new(
            producer.clone(),
            oplog.clone(),
            identity.invocation.clone(),
            [(1, handle.clone(), SessionStreamRoleV1::Input)],
        )
        .with_consumer_journal(Arc::new(RecordingConsumerJournal {
            oplog: oplog.clone(),
            commits: commits.clone(),
        }));
        let mut consumer = DurableInputProducer::new(
            streams
                .endpoint(handle.clone(), 0, SessionStreamRoleV1::Input)
                .await
                .unwrap(),
        );
        consumer.begin_receive();
        let (write_result, receive_result) = tokio::join!(
            producer.write_items(
                handle.stream_id,
                0,
                StreamItemsPayloadV1::PackedU8(bytes.clone()),
            ),
            consumer.pending.take().unwrap(),
        );
        write_result.unwrap();
        let (_, event, _, journaled, queued) = receive_result.unwrap();
        assert!(!journaled);
        assert!(matches!(
            event.unwrap().payload,
            CommittedProducerStreamEventPayloadV1::PackedU8(0)
        ));
        assert_eq!(queued.len(), bytes.len() - 1);
        assert!(queued.iter().all(|event| matches!(
            &event.payload,
            CommittedProducerStreamEventPayloadV1::PackedU8(_)
        )));
        assert_eq!(commits.load(Ordering::Relaxed), 1);

        let (history, _, next_ordinal, terminal) =
            streams.consumer_history(handle.stream_id).await.unwrap();
        assert_eq!(history.len(), bytes.len());
        assert_eq!(next_ordinal, bytes.len() as u64);
        assert!(!terminal);
        assert_eq!(
            history
                .into_iter()
                .map(|event| match event.payload {
                    CommittedProducerStreamEventPayloadV1::PackedU8(byte) => byte,
                    other => panic!("expected packed-u8 history, got {other:?}"),
                })
                .collect::<Vec<_>>(),
            bytes
        );

        let reloaded = DurableStreamProducer::load(
            oplog,
            identity.environment_id,
            identity.agent_id,
            identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        reloaded
            .append_session_record(StreamSessionRecordV1::ConsumerTerminal(
                StreamConsumerTerminalRecordV1 {
                    format_version: 1,
                    session_key: identity.invocation,
                    stream_id: handle.stream_id,
                    source_offset: StreamOffsetV1::new(OplogIndex::from_u64(100), 0),
                    consumer_read_ordinal: 2048,
                    terminal: StreamConsumerTerminalV1::End(StreamEndResultV1::Ok),
                },
            ))
            .await
            .expect("reloading must advance the consumer ordinal by every packed byte");
    }

    #[test]
    async fn dropping_unfinished_input_producer_cancels_the_durable_source_after_cleanup_drain() {
        check_dropped_input_cancellation(false).await;
    }

    #[test]
    async fn dropping_unread_input_resource_cancels_the_durable_source_after_cleanup_drain() {
        check_dropped_input_cancellation(true).await;
    }

    async fn check_dropped_input_cancellation(unread_resource: bool) {
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
                    root_kind: StreamRootKindV1::MethodResult,
                    recursive_value_path: Vec::new(),
                },
                StreamSourceKindV1::InvocationOutput,
            ))
            .await
            .unwrap()
            .value;
        let attempt_id = AttemptId::fresh();
        let attachment_id = AttachmentId::primary(
            identity.environment_id,
            &identity.agent_id,
            &identity.invocation.idempotency_key,
        )
        .unwrap();
        let pending_invocation_oplog_index = append_prepared_pending(
            producer.as_ref(),
            oplog.as_ref(),
            &identity,
            attachment_id,
            attempt_id,
            &handle,
            SessionStreamRoleV1::Output,
        )
        .await;
        producer
            .append_session_record(StreamSessionRecordV1::Attached(
                StreamSessionAttachedRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: identity.invocation.clone(),
                    attachment_id,
                    attempt_id,
                    epoch: 1,
                    pending_invocation_oplog_index,
                },
            ))
            .await
            .unwrap();
        let streams = DurableSessionStreams::new(
            producer.clone(),
            oplog.clone(),
            identity.invocation,
            [(7, handle.clone(), SessionStreamRoleV1::Output)],
        )
        .with_consumer_journal(Arc::new(TestConsumerJournal(oplog.clone())))
        .with_attachment(1, attempt_id);
        let source_cancelled = tokio_util::sync::CancellationToken::new();
        producer.register_source_cancellation(handle.stream_id, source_cancelled.clone());
        let (drop_event_sink, mut drop_events) = mpsc::unbounded_channel();
        let endpoint = streams
            .endpoint(handle.clone(), 0, SessionStreamRoleV1::Output)
            .await
            .unwrap();
        if unread_resource {
            let stream = SchemaValueStream::from_host_endpoint(endpoint);
            let moved = stream.take_for_transfer().unwrap();
            DurableInputProducer::drop_unread(stream, drop_event_sink.clone(), Arc::new(|| false));
            assert!(drop_events.try_recv().is_err());
            assert!(!source_cancelled.is_cancelled());
            DurableInputProducer::drop_unread(moved, drop_event_sink, Arc::new(|| false));
        } else {
            drop(
                DurableInputProducer::new(endpoint)
                    .with_drop_cleanup(drop_event_sink, Arc::new(|| false)),
            );
        }
        let cancellation = match drop_events.recv().await.unwrap() {
            DropEvent::CancelDroppedDurableInput { cancellation } => cancellation,
            event => panic!("unexpected drop event: {event:?}"),
        };
        cancellation.cancel().await.unwrap();
        assert!(source_cancelled.is_cancelled());

        let current = oplog.current_oplog_index().await;
        let mut intents = 0;
        let mut cancellations = 0;
        for (_, entry) in oplog
            .read_exact(OplogIndex::INITIAL, current.as_u64())
            .await
        {
            match entry {
                OplogEntry::StreamSession { record, .. } => {
                    if let StreamSessionRecordV1::ConsumerCancelIntent(intent) =
                        streams.download_record(record).await.unwrap()
                        && intent.stream_id == handle.stream_id
                    {
                        intents += 1;
                        assert_eq!(intent.role, StreamCancelRoleV1::OutputConsumer);
                        assert_eq!(intent.reason, StreamCancelReasonV1::GuestDrop);
                    }
                }
                OplogEntry::StreamCancel { record, .. } => {
                    let cancellation = oplog.download_payload(record).await.unwrap();
                    if cancellation.stream_id == handle.stream_id {
                        cancellations += 1;
                        assert_eq!(cancellation.role, StreamCancelRoleV1::OutputConsumer);
                        assert_eq!(cancellation.reason, StreamCancelReasonV1::GuestDrop);
                    }
                }
                _ => {}
            }
        }
        assert_eq!(intents, 1);
        assert_eq!(cancellations, 1);
    }

    #[test]
    async fn dropped_input_cancellation_is_skipped_once_the_attachment_is_fenced() {
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
                    root_kind: StreamRootKindV1::MethodResult,
                    recursive_value_path: Vec::new(),
                },
                StreamSourceKindV1::InvocationOutput,
            ))
            .await
            .unwrap()
            .value;
        let attempt_id = AttemptId::fresh();
        let attachment_id = AttachmentId::primary(
            identity.environment_id,
            &identity.agent_id,
            &identity.invocation.idempotency_key,
        )
        .unwrap();
        let pending_invocation_oplog_index = append_prepared_pending(
            producer.as_ref(),
            oplog.as_ref(),
            &identity,
            attachment_id,
            attempt_id,
            &handle,
            SessionStreamRoleV1::Output,
        )
        .await;
        producer
            .append_session_record(StreamSessionRecordV1::Attached(
                StreamSessionAttachedRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: identity.invocation.clone(),
                    attachment_id,
                    attempt_id,
                    epoch: 1,
                    pending_invocation_oplog_index,
                },
            ))
            .await
            .unwrap();
        let streams = DurableSessionStreams::new(
            producer.clone(),
            oplog.clone(),
            identity.invocation,
            [(7, handle.clone(), SessionStreamRoleV1::Output)],
        )
        .with_consumer_journal(Arc::new(TestConsumerJournal(oplog.clone())))
        .with_attachment(1, attempt_id);
        let source_cancelled = tokio_util::sync::CancellationToken::new();
        producer.register_source_cancellation(handle.stream_id, source_cancelled.clone());
        let (drop_event_sink, mut drop_events) = mpsc::unbounded_channel();
        let input = DurableInputProducer::new(
            streams
                .endpoint(handle.clone(), 0, SessionStreamRoleV1::Output)
                .await
                .unwrap(),
        )
        .with_drop_cleanup(drop_event_sink, Arc::new(|| false));

        drop(input);
        let cancellation = match drop_events.recv().await.unwrap() {
            DropEvent::CancelDroppedDurableInput { cancellation } => cancellation,
            event => panic!("unexpected drop event: {event:?}"),
        };
        assert!(streams.detach_current().await.unwrap());
        assert!(streams.ensure_current_attachment().await.is_err());
        let before_cancellation = oplog.current_oplog_index().await;

        cancellation.cancel().await.unwrap();

        assert!(!source_cancelled.is_cancelled());
        assert_eq!(oplog.current_oplog_index().await, before_cancellation);
    }

    #[test]
    async fn guest_authored_input_cancellation_targets_the_current_attachment_epoch() {
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
        let start_attempt_id = AttemptId::fresh();
        let pending_invocation_oplog_index = append_prepared_pending(
            producer.as_ref(),
            oplog.as_ref(),
            &identity,
            attachment_id,
            start_attempt_id,
            &handle,
            SessionStreamRoleV1::Input,
        )
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
        let transport = DurableSessionStreams::new(
            producer.clone(),
            oplog.clone(),
            identity.invocation.clone(),
            [(7, handle.clone(), SessionStreamRoleV1::Input)],
        )
        .with_consumer_journal(Arc::new(TestConsumerJournal(oplog.clone())))
        .with_attachment(1, start_attempt_id);
        let guest = DurableSessionStreams::new(
            producer.clone(),
            oplog.clone(),
            identity.invocation.clone(),
            [(7, handle.clone(), SessionStreamRoleV1::Input)],
        )
        .with_consumer_journal(Arc::new(TestConsumerJournal(oplog.clone())));
        let source_cancelled = tokio_util::sync::CancellationToken::new();
        producer.register_source_cancellation(handle.stream_id, source_cancelled.clone());

        assert!(transport.detach_current().await.unwrap());
        let resume_attempt_id = AttemptId::fresh();
        transport
            .commit_resume_attempt(StreamSessionResumeAttemptRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                attempt: ResumeAttemptDescriptorV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    operation: StreamResumeOperationV1::Resume,
                    session_key: identity.invocation.clone(),
                    attachment_id,
                    expected_callee_fingerprint: identity.fingerprint,
                    attempt_id: resume_attempt_id,
                    expected_epoch: 1,
                    effective_identity: vec![3],
                    cursors: Vec::new(),
                    live_join_buffer_events: 8,
                },
                accepted_epoch: 2,
            })
            .await
            .unwrap();
        assert!(transport.ensure_current_attachment().await.is_err());
        assert!(guest.ensure_current_attachment().await.is_err());

        guest
            .cancel_stream(
                7,
                StreamCancelRoleV1::InputProducer,
                StreamCancelReasonV1::GuestDrop,
                Some("guest dropped input readable end".to_string()),
                None,
            )
            .await
            .unwrap();

        assert!(source_cancelled.is_cancelled());
        let current = oplog.current_oplog_index().await;
        let mut intents = Vec::new();
        for (_, entry) in oplog
            .read_exact(OplogIndex::INITIAL, current.as_u64())
            .await
        {
            if let OplogEntry::StreamSession { record, .. } = entry
                && let StreamSessionRecordV1::ConsumerCancelIntent(record) =
                    guest.download_record(record).await.unwrap()
            {
                intents.push(record);
            }
        }
        assert_eq!(intents.len(), 1);
        assert_eq!(intents[0].stream_id, handle.stream_id);
        assert_eq!(intents[0].epoch, 2);
        assert_eq!(intents[0].role, StreamCancelRoleV1::InputProducer);
        assert_eq!(intents[0].reason, StreamCancelReasonV1::GuestDrop);

        for _ in 0..2050 {
            oplog.add(OplogEntry::interrupted()).await;
        }
        drop(guest.current_control_metadata().await.unwrap());
        oplog.take_read_ranges();
        let after_cancellation = oplog.current_oplog_index().await;
        guest
            .cancel_stream(
                7,
                StreamCancelRoleV1::InputProducer,
                StreamCancelReasonV1::GuestDrop,
                Some("guest dropped input readable end".to_string()),
                None,
            )
            .await
            .unwrap();
        assert_eq!(oplog.current_oplog_index().await, after_cancellation);
        assert!(
            oplog.take_read_ranges().is_empty(),
            "repeated cancellation must reuse the indexed intent"
        );
    }

    #[test]
    async fn dropping_input_producer_during_runtime_teardown_does_not_schedule_cancellation() {
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
                    root_kind: StreamRootKindV1::MethodResult,
                    recursive_value_path: Vec::new(),
                },
                StreamSourceKindV1::InvocationOutput,
            ))
            .await
            .unwrap()
            .value;
        let streams = DurableSessionStreams::new(
            producer,
            oplog,
            identity.invocation,
            [(7, handle.clone(), SessionStreamRoleV1::Output)],
        );
        let (drop_event_sink, mut drop_events) = mpsc::unbounded_channel();
        let input = DurableInputProducer::new(
            streams
                .endpoint(handle, 0, SessionStreamRoleV1::Output)
                .await
                .unwrap(),
        )
        .with_drop_cleanup(drop_event_sink, Arc::new(|| true));

        drop(input);
        assert!(drop_events.try_recv().is_err());
    }

    #[test]
    async fn closed_foreign_journal_replays_after_source_finalization_and_epoch_change() {
        let source = identity();
        let source_oplog = Arc::new(TestOplog::default());
        let producer = DurableStreamProducer::load(
            source_oplog.clone(),
            source.environment_id,
            source.agent_id.clone(),
            source.fingerprint,
            None,
        )
        .await
        .unwrap();
        let handle = producer
            .register(registration(
                &source,
                StreamRegistrationCoordinateV1::Root {
                    invocation_id: source.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodResult,
                    recursive_value_path: Vec::new(),
                },
                StreamSourceKindV1::InvocationOutput,
            ))
            .await
            .unwrap()
            .value;
        let mut consumer = identity();
        consumer.agent_id.agent_id = "closed-journal-consumer".into();
        consumer.fingerprint = AgentFingerprint::new();
        consumer.invocation.callee = consumer.agent_id.clone();
        consumer.invocation.callee_fingerprint = consumer.fingerprint;
        let oplog = Arc::new(TestOplog::default());
        let local = DurableStreamProducer::load(
            oplog.clone(),
            consumer.environment_id,
            consumer.agent_id.clone(),
            consumer.fingerprint,
            None,
        )
        .await
        .unwrap();
        let streams =
            DurableSessionStreams::new(local, oplog.clone(), source.invocation.clone(), [])
                .with_consumer_invocation(consumer.invocation.clone())
                .with_consumer_journal(Arc::new(TestConsumerJournal(oplog.clone())));
        let attachment = streams.attachment_key(&handle, 1).unwrap();
        let mapping = StreamSessionMappingRecordV1 {
            transport_stream_id: 1,
            handle: handle.clone(),
            role: SessionStreamRoleV1::Output,
        };
        for record in [
            StreamSessionRecordV1::TopologyPrepared(StreamTopologyPreparedRecordV1 {
                format_version: 1,
                session_key: source.invocation.clone(),
                attachment: attachment.clone(),
                mapping: mapping.clone(),
            }),
            StreamSessionRecordV1::TopologyActivated(StreamTopologyActivatedRecordV1 {
                format_version: 1,
                session_key: source.invocation.clone(),
                attachment: attachment.clone(),
                mapping: mapping.clone(),
            }),
            StreamSessionRecordV1::Mapping(
                golem_common::model::durable_stream::StreamSessionMappingUpdateRecordV1 {
                    format_version: 1,
                    session_key: source.invocation.clone(),
                    mapping: mapping.clone(),
                },
            ),
        ] {
            assert!(record.has_supported_format());
            streams.append_record(record).await;
        }
        producer
            .prepare_attachment(attachment.clone(), 100)
            .await
            .unwrap();
        producer
            .activate_attachment(attachment.clone(), 100)
            .await
            .unwrap();
        producer
            .end(handle.stream_id, 0, StreamEndResultV1::Ok)
            .await
            .unwrap();
        let source_offset = producer
            .input_high_water(handle.stream_id)
            .await
            .unwrap()
            .unwrap()
            .resulting_offset;
        streams
            .append_record(StreamSessionRecordV1::ConsumerTerminal(
                StreamConsumerTerminalRecordV1 {
                    format_version: 1,
                    session_key: source.invocation.clone(),
                    stream_id: handle.stream_id,
                    source_offset,
                    consumer_read_ordinal: 0,
                    terminal: StreamConsumerTerminalV1::End(StreamEndResultV1::Ok),
                },
            ))
            .await;
        streams.commit_consumer_journal().await.unwrap();
        producer.finalize_attachment(attachment.clone(), golem_common::model::durable_stream::StreamAttachmentFinalizationReasonV1::ConsumerFinalized, 101).await.unwrap();
        let mut next_attachment = attachment;
        next_attachment.epoch = 2;
        assert!(
            producer
                .prepare_attachment(next_attachment, 102)
                .await
                .is_err()
        );
        drop(streams);
        drop(producer);
        drop(source_oplog);

        // Reconstruct after the journal commit but before delivering the terminal to the guest.
        let local = DurableStreamProducer::load(
            oplog.clone(),
            consumer.environment_id,
            consumer.agent_id,
            consumer.fingerprint,
            None,
        )
        .await
        .unwrap();
        let restarted = DurableSessionStreams::new(local, oplog.clone(), source.invocation, [])
            .with_consumer_invocation(consumer.invocation)
            .with_attachment(2, AttemptId(Uuid::new_v4()))
            .with_consumer_journal(Arc::new(TestConsumerJournal(oplog)));
        restarted.recover_session_mappings().await.unwrap();
        assert!(
            restarted
                .has_journaled_consumer_terminal(&mapping)
                .await
                .unwrap()
        );
        let mut wrong_mapping = mapping;
        wrong_mapping.handle.expected_producer_fingerprint = AgentFingerprint::new();
        assert!(
            restarted
                .has_journaled_consumer_terminal(&wrong_mapping)
                .await
                .is_err()
        );
        let endpoint = restarted
            .endpoint(handle, 0, SessionStreamRoleV1::Output)
            .await
            .unwrap();
        assert!(
            endpoint.reader.is_none(),
            "closed replay must not open a remote source"
        );
        let mut replay = DurableInputProducer::new(endpoint);
        replay.begin_receive();
        let (_, event, _, journaled, _) = replay.pending.take().unwrap().await.unwrap();
        assert!(journaled);
        assert!(matches!(
            event.unwrap().payload,
            CommittedProducerStreamEventPayloadV1::End(StreamEndResultV1::Ok)
        ));
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
        let (_, event, _, journaled, _) = replay.pending.take().unwrap().await.unwrap();
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
    async fn resume_cursors_cannot_name_input_streams() {
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

        let error = streams
            .validate_resume_cursors(
                &[golem_common::model::durable_stream::StreamResumeCursorV1 {
                    stream_id: handle.stream_id,
                    last_observed_offset: None,
                }],
            )
            .await
            .unwrap_err();
        assert!(error.contains("no output mapping"));
    }

    #[test]
    async fn output_resume_cursor_is_not_shadowed_by_same_handle_input_mapping() {
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
        let cursor = golem_common::model::durable_stream::StreamResumeCursorV1 {
            stream_id: handle.stream_id,
            last_observed_offset: None,
        };

        for _ in 0..32 {
            let streams = DurableSessionStreams::new(
                producer.clone(),
                oplog.clone(),
                identity.invocation.clone(),
                [
                    (1, handle.clone(), SessionStreamRoleV1::Input),
                    (2, handle.clone(), SessionStreamRoleV1::Output),
                ],
            );
            assert!(
                streams
                    .validate_resume_cursors(std::slice::from_ref(&cursor))
                    .await
                    .is_ok(),
                "the output mapping must authorize its cursor regardless of the same handle's input mapping"
            );
        }
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
        let before_stale_cancellation = oplog.current_oplog_index().await;
        let error = epoch1
            .cancel_stream(
                7,
                StreamCancelRoleV1::InputProducer,
                StreamCancelReasonV1::Cancelled,
                Some("deferred cancellation from the old attachment".to_string()),
                None,
            )
            .await
            .unwrap_err();
        assert!(error.contains("StaleEpoch"), "unexpected error: {error}");
        assert_eq!(oplog.current_oplog_index().await, before_stale_cancellation);
        let before_old_detach = oplog.current_oplog_index().await;
        assert!(!epoch1.detach_current().await.unwrap());
        assert_eq!(oplog.current_oplog_index().await, before_old_detach);

        let takeover_attempt_id = AttemptId::fresh();
        let epoch2_monitor = epoch2.clone();
        let epoch2_revoked =
            tokio::spawn(async move { epoch2_monitor.wait_for_attachment_revocation().await });
        epoch2
            .commit_resume_attempt(StreamSessionResumeAttemptRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                attempt: attempt(StreamResumeOperationV1::Takeover, 2, takeover_attempt_id),
                accepted_epoch: 3,
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), epoch2_revoked)
            .await
            .expect("idle attachment did not observe takeover revocation")
            .unwrap()
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
        for (index, entry) in oplog
            .read_exact(OplogIndex::INITIAL, current.as_u64())
            .await
        {
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
            let (_, event, nested, journaled, _) = consumer.pending.take().unwrap().await.unwrap();
            assert!(!journaled);
            assert!(event.is_some());
            assert_eq!(nested.len(), 1);
        }

        let mut persisted_roles = HashMap::new();
        let current = oplog.current_oplog_index().await;
        for (_, entry) in oplog
            .read_exact(OplogIndex::INITIAL, current.as_u64())
            .await
        {
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
            cancellation_owner: None,
            scripted_reads: Mutex::default(),
            pending_read: Mutex::default(),
            read_requests: Mutex::default(),
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
                remote_producer.clone(),
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
                remote_producer.clone(),
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
                remote_producer.clone(),
                120,
            )
            .await
            .unwrap();
        assert_eq!(consumer_oplog.current_oplog_index().await, consumer_length);
        assert_eq!(producer_oplog.current_oplog_index().await, producer_length);

        for _ in 0..2050 {
            consumer_oplog.add(OplogEntry::interrupted()).await;
        }
        assert_eq!(
            streams
                .topology_state(&attachment, Some(&mapping))
                .await
                .unwrap(),
            ConsumerAttachmentStatus::Active
        );
        consumer_oplog.take_read_ranges();
        assert_eq!(
            streams
                .topology_state(&attachment, Some(&mapping))
                .await
                .unwrap(),
            ConsumerAttachmentStatus::Active
        );
        streams.validate_topology_complete().await.unwrap();
        assert!(
            consumer_oplog.take_read_ranges().is_empty(),
            "warm topology checks must not revisit unrelated history"
        );

        let mut projection = streams.current_control_metadata().await.unwrap().clone();
        let initial_size = golem_common::serialization::serialize(&projection)
            .unwrap()
            .len();
        let mut resumed_attachment = attachment.clone();
        for epoch in 2..1002 {
            resumed_attachment.epoch = epoch;
            projection.apply(
                OplogIndex::from_u64(10_000 + epoch * 2),
                &consumer.invocation,
                &StreamSessionRecordV1::TopologyPrepared(StreamTopologyPreparedRecordV1 {
                    format_version: 1,
                    session_key: consumer.invocation.clone(),
                    attachment: resumed_attachment.clone(),
                    mapping: mapping.clone(),
                }),
            );
            assert_eq!(
                projection
                    .topology_status(&resumed_attachment, Some(&mapping))
                    .unwrap(),
                ConsumerAttachmentStatus::Prepared
            );
            projection.apply(
                OplogIndex::from_u64(10_001 + epoch * 2),
                &consumer.invocation,
                &StreamSessionRecordV1::TopologyActivated(StreamTopologyActivatedRecordV1 {
                    format_version: 1,
                    session_key: consumer.invocation.clone(),
                    attachment: resumed_attachment.clone(),
                    mapping: mapping.clone(),
                }),
            );
            assert_eq!(
                projection
                    .topology_status(&resumed_attachment, Some(&mapping))
                    .unwrap(),
                ConsumerAttachmentStatus::Active
            );
        }
        assert_eq!(
            projection
                .topology_status(&attachment, Some(&mapping))
                .unwrap(),
            ConsumerAttachmentStatus::EpochMismatch
        );
        assert!(
            projection.topology_error.is_none(),
            "successive epochs for the same attachment slot are not conflicting topology"
        );
        assert!(
            golem_common::serialization::serialize(&projection)
                .unwrap()
                .len()
                < initial_size + 1024,
            "topology metadata must not grow with resume count"
        );

        let mut conflicting = projection.clone();
        let mut conflicting_attachment = resumed_attachment.clone();
        conflicting_attachment.expected_consumer_fingerprint = AgentFingerprint::new();
        conflicting.apply(
            OplogIndex::from_u64(20_000),
            &consumer.invocation,
            &StreamSessionRecordV1::TopologyPrepared(StreamTopologyPreparedRecordV1 {
                format_version: 1,
                session_key: consumer.invocation.clone(),
                attachment: conflicting_attachment,
                mapping: mapping.clone(),
            }),
        );
        assert!(
            conflicting
                .topology_status(&resumed_attachment, Some(&mapping))
                .is_err(),
            "a conflicting same-epoch identity must not leave the old topology active"
        );

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
                    remote_producer.clone(),
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
                    remote_producer.clone(),
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
                    producer.clone(),
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
            .activate_forwarded_mapping(attachment.clone(), mapping.clone(), producer.clone(), 110)
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
        let root_ended = producer
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

        let (packed_responses, mut packed_receiver) = mpsc::channel(2);
        streams
            .pump_output_stream_from(
                *nested_transport_stream_id,
                nested.clone(),
                None,
                &packed_responses,
            )
            .await
            .unwrap();
        let Some(invocation_response::Response::OutputItem(packed)) =
            packed_receiver.recv().await.unwrap().response
        else {
            panic!("packed durable bytes must produce an output item")
        };
        assert_eq!(packed.producer_sequence, 0);
        assert_eq!(packed.logical_item_count, 2);
        assert_eq!(packed.packed_u8, vec![10, 11]);
        assert!(packed.value.is_none());
        assert!(packed.new_stream_mappings.is_empty());
        assert_eq!(
            packed.durable_offset,
            nested_written.value[1].as_bytes().to_vec()
        );
        let Some(invocation_response::Response::OutputEnd(end)) =
            packed_receiver.recv().await.unwrap().response
        else {
            panic!("packed durable bytes must preserve their terminal")
        };
        assert_eq!(end.producer_sequence, 2);

        let mut lone_request = registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: session_key.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: vec![StreamValuePathStepV1::RecordField(1)],
            },
            StreamSourceKindV1::InvocationOutput,
        );
        lone_request.session_mapping = Some(StreamSessionMappingV1 {
            session_key: session_key.clone(),
            attachment_id,
            role: SessionStreamRoleV1::Output,
        });
        let lone = producer.register(lone_request).await.unwrap().value;
        let lone_transport_stream_id = streams.allocate_transport_stream_id().unwrap();
        streams
            .insert_mapping(
                lone_transport_stream_id,
                lone.clone(),
                SessionStreamRoleV1::Output,
            )
            .unwrap();
        let (lone_responses, mut lone_receiver) = mpsc::channel(2);
        let lone_streams = streams.clone();
        let lone_for_pump = lone.clone();
        let lone_pump = tokio::spawn(async move {
            lone_streams
                .pump_output_stream_from(
                    lone_transport_stream_id,
                    lone_for_pump,
                    None,
                    &lone_responses,
                )
                .await
        });
        let lone_written = producer
            .write_items(lone.stream_id, 0, StreamItemsPayloadV1::PackedU8(vec![99]))
            .await
            .unwrap();
        let lone_item = tokio::time::timeout(
            PACKED_U8_OUTPUT_FLUSH_DELAY + Duration::from_millis(100),
            lone_receiver.recv(),
        )
        .await
        .expect("a lone packed byte exceeded the bounded output flush delay")
        .unwrap();
        let Some(invocation_response::Response::OutputItem(lone_item)) = lone_item.response else {
            panic!("a lone packed byte must produce an output item")
        };
        assert_eq!(lone_item.packed_u8, vec![99]);
        assert_eq!(lone_item.logical_item_count, 1);
        assert_eq!(
            lone_item.durable_offset,
            lone_written.value[0].as_bytes().to_vec()
        );
        producer
            .end(lone.stream_id, 1, StreamEndResultV1::Ok)
            .await
            .unwrap();
        assert!(matches!(
            lone_receiver.recv().await.unwrap().response,
            Some(invocation_response::Response::OutputEnd(_))
        ));
        lone_pump.await.unwrap().unwrap();

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
            .pump_output_streams_from(&cursors, &[7], &[7, nested_transport_stream_id], &responses)
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

        let terminal_parent_cursors = HashMap::from([
            (root.stream_id, Some(root_ended.value)),
            (nested.stream_id, Some(nested_written.value[0])),
        ]);
        let (responses, mut receiver) = mpsc::channel(8);
        tokio::time::timeout(
            Duration::from_secs(1),
            restarted.pump_output_streams_from(
                &terminal_parent_cursors,
                &[7],
                &[7, nested_transport_stream_id],
                &responses,
            ),
        )
        .await
        .expect("resume from a parent terminal cursor must complete")
        .unwrap();
        drop(responses);
        let mut replayed_nested = Vec::new();
        let mut ended_streams = HashSet::new();
        while let Some(response) = receiver.recv().await {
            match response.response {
                Some(invocation_response::Response::OutputItem(item)) => {
                    replayed_nested.push((item.transport_stream_id, item.packed_u8));
                }
                Some(invocation_response::Response::OutputEnd(end)) => {
                    ended_streams.insert(end.transport_stream_id);
                }
                other => panic!("unexpected terminal-parent resume response: {other:?}"),
            }
        }
        assert_eq!(
            replayed_nested,
            vec![(nested_transport_stream_id, vec![11])]
        );
        assert_eq!(ended_streams, HashSet::from([nested_transport_stream_id]));
    }

    #[test]
    async fn resumed_foreign_parent_and_nested_output_cursors_use_the_accepted_epoch() {
        let consumer = identity();
        let producer_identity = TestIdentity {
            environment_id: EnvironmentId(Uuid::from_u128(71)),
            agent_id: AgentId {
                component_id: ComponentId(Uuid::from_u128(72)),
                agent_id: "resumed-foreign-producer".to_string(),
            },
            fingerprint: AgentFingerprint(Uuid::from_u128(73)),
            invocation: StreamInvocationIdV1 {
                callee_environment_id: EnvironmentId(Uuid::from_u128(71)),
                callee: AgentId {
                    component_id: ComponentId(Uuid::from_u128(72)),
                    agent_id: "resumed-foreign-producer".to_string(),
                },
                callee_fingerprint: AgentFingerprint(Uuid::from_u128(73)),
                idempotency_key: IdempotencyKey::new(
                    "resumed-foreign-producer-invocation".to_string(),
                ),
            },
        };
        let producer_oplog = Arc::new(TestOplog::default());
        let remote_producer = DurableStreamProducer::load(
            producer_oplog,
            producer_identity.environment_id,
            producer_identity.agent_id.clone(),
            producer_identity.fingerprint,
            None,
        )
        .await
        .unwrap();
        let root = remote_producer
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
        let root_written = remote_producer
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
                vec![registration(
                    &producer_identity,
                    StreamRegistrationCoordinateV1::Nested {
                        parent_stream_id: root.stream_id,
                        parent_producer_sequence: 0,
                        recursive_value_path: Vec::new(),
                    },
                    StreamSourceKindV1::Nested,
                )],
            )
            .await
            .unwrap();
        remote_producer
            .end(root.stream_id, 1, StreamEndResultV1::Ok)
            .await
            .unwrap();
        let nested = remote_producer
            .nested_handles(root.stream_id, 0)
            .await
            .unwrap()[0]
            .clone();
        let nested_written = remote_producer
            .write_items(
                nested.stream_id,
                0,
                StreamItemsPayloadV1::PackedU8(vec![10, 11]),
            )
            .await
            .unwrap();
        remote_producer
            .end(nested.stream_id, 2, StreamEndResultV1::Ok)
            .await
            .unwrap();

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
        let attachment_id = AttachmentId::primary(
            consumer.environment_id,
            &consumer.agent_id,
            &consumer.invocation.idempotency_key,
        )
        .unwrap();
        let start_attempt_id = AttemptId::fresh();
        let streams = DurableSessionStreams::new(
            consumer_producer,
            consumer_oplog.clone(),
            consumer.invocation.clone(),
            [],
        )
        .with_consumer_journal(Arc::new(TestConsumerJournal(consumer_oplog.clone())))
        .with_rpc(Arc::new(AttachedProducerRpc {
            producer: remote_producer.clone(),
            cancellation_owner: None,
            scripted_reads: Mutex::default(),
            pending_read: Mutex::default(),
            read_requests: Mutex::default(),
        }))
        .with_auth_ctx(AuthCtx::System);
        streams
            .append_record(StreamSessionRecordV1::Prepared(
                StreamSessionPreparedRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    attempt: StartAttemptDescriptorV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: consumer.invocation.clone(),
                        attachment_id,
                        expected_callee_fingerprint: consumer.fingerprint,
                        attempt_id: start_attempt_id,
                        invocation: PersistedStreamInvocationDescriptorV1 {
                            format_version: DURABLE_STREAM_FORMAT_VERSION,
                            session_key: consumer.invocation.clone(),
                            target_component_revision: ComponentRevision::INITIAL,
                            method_name: "resume-foreign".to_string(),
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
                StreamSessionAttachedRecordV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    session_key: consumer.invocation.clone(),
                    attachment_id,
                    attempt_id: start_attempt_id,
                    epoch: 1,
                    pending_invocation_oplog_index,
                },
            ))
            .await;
        let epoch1 = streams.with_attachment(1, start_attempt_id);
        let root_mapping = StreamSessionMappingRecordV1 {
            transport_stream_id: 17,
            handle: root.clone(),
            role: SessionStreamRoleV1::Output,
        };
        let nested_mapping = StreamSessionMappingRecordV1 {
            transport_stream_id: 18,
            handle: nested.clone(),
            role: SessionStreamRoleV1::Output,
        };
        for mapping in [&root_mapping, &nested_mapping] {
            let attachment = epoch1.attachment_key(&mapping.handle, 1).unwrap();
            epoch1
                .activate_forwarded_mapping(
                    attachment,
                    mapping.clone(),
                    remote_producer.clone(),
                    100,
                )
                .await
                .unwrap();
        }
        let resumed_attempt_id = AttemptId::fresh();
        epoch1
            .commit_resume_attempt(StreamSessionResumeAttemptRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                attempt: ResumeAttemptDescriptorV1 {
                    format_version: DURABLE_STREAM_FORMAT_VERSION,
                    operation: StreamResumeOperationV1::Takeover,
                    session_key: consumer.invocation.clone(),
                    attachment_id,
                    expected_callee_fingerprint: consumer.fingerprint,
                    attempt_id: resumed_attempt_id,
                    expected_epoch: 1,
                    effective_identity: vec![3],
                    cursors: Vec::new(),
                    live_join_buffer_events: 8,
                },
                accepted_epoch: 2,
            })
            .await
            .unwrap();
        let resumed = epoch1.with_attachment(2, resumed_attempt_id);
        for mapping in [&root_mapping, &nested_mapping] {
            let attachment = resumed.attachment_key(&mapping.handle, 2).unwrap();
            resumed
                .activate_forwarded_mapping(
                    attachment,
                    mapping.clone(),
                    remote_producer.clone(),
                    200,
                )
                .await
                .unwrap();
        }

        let cursors = HashMap::from([
            (root.stream_id, Some(root_written.value[0])),
            (nested.stream_id, Some(nested_written.value[0])),
        ]);
        let (responses, mut receiver) = mpsc::channel(8);
        resumed
            .pump_output_streams_from(
                &cursors,
                &[root_mapping.transport_stream_id],
                &[
                    root_mapping.transport_stream_id,
                    nested_mapping.transport_stream_id,
                ],
                &responses,
            )
            .await
            .unwrap();
        drop(responses);
        let mut replayed_items = Vec::new();
        let mut ended_streams = HashSet::new();
        while let Some(response) = receiver.recv().await {
            match response.response {
                Some(invocation_response::Response::OutputItem(item)) => {
                    replayed_items.push((item.transport_stream_id, item.packed_u8));
                }
                Some(invocation_response::Response::OutputEnd(end)) => {
                    ended_streams.insert(end.transport_stream_id);
                }
                other => panic!("unexpected resumed foreign output response: {other:?}"),
            }
        }
        assert_eq!(
            replayed_items,
            vec![(nested_mapping.transport_stream_id, vec![11])]
        );
        assert_eq!(
            ended_streams,
            HashSet::from([
                root_mapping.transport_stream_id,
                nested_mapping.transport_stream_id,
            ])
        );
    }

    #[test]
    async fn session_control_metadata_pages_history_and_reads_only_raw_suffix_after_warmup() {
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
        let streams =
            DurableSessionStreams::new(producer, oplog.clone(), identity.invocation.clone(), []);
        for _ in 0..2050 {
            oplog.add(OplogEntry::interrupted()).await;
        }
        assert!(
            streams
                .session_root_output_mapping_ids()
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            oplog.take_read_ranges(),
            vec![
                (OplogIndex::INITIAL, 1024),
                (OplogIndex::from_u64(1025), 1024),
                (OplogIndex::from_u64(2049), 2),
            ]
        );
        assert!(streams.persisted_finished().await.unwrap().is_none());
        assert!(streams.remote_result_record().await.unwrap().is_none());
        assert!(oplog.take_read_ranges().is_empty());

        let attempt_id = AttemptId::fresh();
        let index = oplog
            .add(OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                record: OplogPayload::Inline(Box::new(StreamSessionRecordV1::CallerAttempt(
                    StreamCallerAttemptRecordV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: identity.invocation,
                        attempt_id,
                    },
                ))),
            })
            .await;
        // No commit: another local append must already be visible.
        assert_eq!(streams.caller_attempt_id().await.unwrap(), attempt_id);
        assert_eq!(oplog.take_read_ranges(), vec![(index, 1)]);
        oplog.commit(CommitLevel::Always).await;
        assert_eq!(
            streams.clone().caller_attempt_id().await.unwrap(),
            attempt_id
        );
        assert!(oplog.take_read_ranges().is_empty());
    }

    #[test]
    async fn session_mapping_recovery_pages_once_and_shares_coverage_with_clones() {
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
        let streams = DurableSessionStreams::new(producer, oplog.clone(), identity.invocation, []);
        for _ in 0..2050 {
            oplog.add(OplogEntry::interrupted()).await;
        }
        streams.recover_session_mappings().await.unwrap();
        assert_eq!(
            oplog.take_read_ranges().iter().map(|(_, n)| n).sum::<u64>(),
            2050
        );
        streams.clone().recover_session_mappings().await.unwrap();
        assert!(oplog.take_read_ranges().is_empty());
        let next = oplog.add(OplogEntry::interrupted()).await;
        streams.recover_session_mappings().await.unwrap();
        assert_eq!(oplog.take_read_ranges(), vec![(next, 1)]);
    }

    #[test]
    async fn suspended_control_metadata_reader_does_not_reserve_the_shared_permit() {
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
        let streams = DurableSessionStreams::new(producer, oplog, identity.invocation, []);

        let guard = streams.control_metadata.lock().await;
        let mut suspended = Box::pin(streams.current_control_metadata());
        assert!(futures::poll!(&mut suspended).is_pending());
        drop(guard);

        let metadata =
            tokio::time::timeout(Duration::from_secs(1), streams.current_control_metadata())
                .await
                .expect("a suspended reader must not block an independently polled lookup")
                .unwrap();
        drop(metadata);
        drop(suspended);
    }

    #[test]
    async fn invocation_winner_drops_queued_persisted_result_waiter_before_followup() {
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
        let streams = DurableSessionStreams::new(producer, oplog, identity.invocation, []);

        let guard = streams.control_metadata.lock().await;
        let mut invocation = Box::pin(streams.recover_nested_input_mappings());
        assert!(futures::poll!(&mut invocation).is_pending());
        let mut result = Box::pin(streams.wait_persisted_result());
        assert!(futures::poll!(&mut result).is_pending());
        drop(guard);

        enum Winner {
            Invocation,
            Result,
        }
        let winner = tokio::select! {
            biased;
            _ = &mut result => Winner::Result,
            recovered = &mut invocation => {
                recovered.unwrap();
                Winner::Invocation
            },
        };
        assert!(matches!(winner, Winner::Invocation));

        drop(result);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), streams.persisted_result())
                .await
                .expect("the invocation follow-up must not remain behind the abandoned waiter")
                .unwrap(),
            None
        );
    }

    #[test]
    async fn finalization_after_retirement_requires_matching_committed_finished() {
        struct FinishedJournal {
            index: Option<OplogIndex>,
            hide_first: bool,
            reads: AtomicU64,
        }

        #[async_trait::async_trait]
        impl DurableStreamConsumerJournal for FinishedJournal {
            async fn commit(&self) -> Result<(), String> {
                panic!("a repeated finalization must not commit buffered entries")
            }

            async fn committed_finished_index(
                &self,
                _session: &StreamSessionKeyV1,
            ) -> Result<Option<OplogIndex>, String> {
                let first = self.reads.fetch_add(1, Ordering::Relaxed) == 0;
                Ok(if first && self.hide_first {
                    None
                } else {
                    self.index
                })
            }
        }

        for (committed, hide_first, same_fingerprint) in [
            (true, false, true),
            (true, true, true),
            (false, false, true),
            (true, false, false),
        ] {
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
            let mut finished_key = identity.invocation.clone();
            if !same_fingerprint {
                finished_key.callee_fingerprint = AgentFingerprint::default();
                assert_ne!(finished_key, identity.invocation);
            }
            let index = oplog
                .add(OplogEntry::StreamSession {
                    timestamp: Timestamp::now_utc(),
                    entity_parent_start_index: None,
                    record: OplogPayload::Inline(Box::new(StreamSessionRecordV1::Finished(
                        StreamSessionFinishedRecordV1 {
                            format_version: DURABLE_STREAM_FORMAT_VERSION,
                            session_key: finished_key,
                            result: Err(vec![1, 2, 3]),
                        },
                    ))),
                })
                .await;
            if committed {
                oplog.commit(CommitLevel::Always).await;
            }
            producer.poison();
            let journal = Arc::new(FinishedJournal {
                index: committed.then_some(index),
                hide_first,
                reads: AtomicU64::new(0),
            });
            let streams =
                DurableSessionStreams::new(producer, oplog.clone(), identity.invocation, [])
                    .with_consumer_journal(journal.clone());
            let result = streams.fail_invocation("execution failed".into()).await;
            assert_eq!(result.is_ok(), committed && same_fingerprint, "{result:?}");
            assert_eq!(oplog.current_oplog_index().await, index);
            assert_eq!(
                journal.reads.load(Ordering::Relaxed),
                if hide_first || !committed { 2 } else { 1 }
            );
            if !committed {
                assert_eq!(oplog.commit(CommitLevel::Always).await.len(), 1);
            }
        }
    }

    #[test]
    async fn finished_in_raw_suffix_is_visible_and_cached() {
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
        let streams =
            DurableSessionStreams::new(producer, oplog.clone(), identity.invocation.clone(), []);
        let index = oplog
            .add(OplogEntry::StreamSession {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                record: OplogPayload::Inline(Box::new(StreamSessionRecordV1::Finished(
                    StreamSessionFinishedRecordV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: identity.invocation,
                        result: Err(vec![1, 2, 3]),
                    },
                ))),
            })
            .await;

        assert_eq!(
            streams.persisted_finished().await.unwrap(),
            Some(Err(vec![1, 2, 3]))
        );
        assert_eq!(oplog.take_read_ranges(), vec![(index, 1)]);
        assert_eq!(
            streams.persisted_finished().await.unwrap(),
            Some(Err(vec![1, 2, 3]))
        );
        assert!(oplog.take_read_ranges().is_empty());
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
    async fn forwarded_root_input_and_direct_result_preserve_the_complete_handle() {
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
            root_kind: StreamRootKindV1::MethodInput,
            recursive_value_path: vec![StreamValuePathStepV1::ListElement(7)],
        };
        let mut request = registration(
            &identity,
            source_coordinate,
            StreamSourceKindV1::AgentHostedInput,
        );
        request.element_schema_fingerprint = fingerprint;
        request.session_mapping = Some(StreamSessionMappingV1 {
            session_key: identity.invocation.clone(),
            attachment_id: AttachmentId::primary(
                identity.environment_id,
                &identity.agent_id,
                &identity.invocation.idempotency_key,
            )
            .unwrap(),
            role: SessionStreamRoleV1::Input,
        });
        let original = producer.register(request).await.unwrap().value;
        let streams = DurableSessionStreams::new(
            producer.clone(),
            oplog.clone(),
            identity.invocation.clone(),
            [(0, original.clone(), SessionStreamRoleV1::Input)],
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
                .unwrap()
                .is_none()
        );

        let result = SchemaValue::Stream(SchemaValueStream::from_host_endpoint(
            streams
                .endpoint(original.clone(), 0, SessionStreamRoleV1::Input)
                .await
                .unwrap(),
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

        streams
            .complete_or_defer_for_forwarded_inputs()
            .await
            .unwrap();
        assert_eq!(streams.persisted_finished().await.unwrap(), None);
        producer
            .end(original.stream_id, 0, StreamEndResultV1::Ok)
            .await
            .unwrap();
        streams
            .complete_or_defer_for_forwarded_inputs()
            .await
            .unwrap();
        assert_eq!(streams.persisted_finished().await.unwrap(), Some(Ok(())));

        assert!(
            producer
                .handle_for_coordinate(&StreamRegistrationCoordinateV1::Root {
                    invocation_id: identity.invocation.clone(),
                    root_kind: StreamRootKindV1::MethodResult,
                    recursive_value_path: Vec::new(),
                })
                .await
                .unwrap()
                .is_none()
        );

        let read_endpoint = streams
            .endpoint(original.clone(), 1, SessionStreamRoleV1::Input)
            .await
            .unwrap();
        assert_eq!(
            read_endpoint.into_forwarded().err().unwrap(),
            "cannot forward a durable input stream after reading from it"
        );

        let mut journaled_endpoint = streams
            .endpoint(original.clone(), 0, SessionStreamRoleV1::Input)
            .await
            .unwrap();
        journaled_endpoint
            .journal
            .push_back(CommittedProducerStreamEventV1 {
                stream_id: original.stream_id,
                producer_sequence: 0,
                offset: golem_common::model::durable_stream::StreamOffsetV1::new(
                    OplogIndex::INITIAL,
                    0,
                ),
                packed_u8_batch_end: None,
                terminal_author: None,
                nested_handles: Vec::new(),
                payload: CommittedProducerStreamEventPayloadV1::PackedU8(0),
            });
        assert_eq!(
            journaled_endpoint.into_forwarded().err().unwrap(),
            "cannot forward a durable input stream after reading from it"
        );
    }

    #[test]
    async fn schema_mismatch_does_not_consume_forwarded_stream() {
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
        let source_element_type = SchemaType::u32();
        let source_graph = SchemaGraph::anonymous(source_element_type.clone());
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
            schema_fingerprint_v1(&source_graph, Some(&source_element_type)).unwrap();
        let handle = producer.register(request).await.unwrap().value;
        let mut second_request = registration(
            &identity,
            StreamRegistrationCoordinateV1::Root {
                invocation_id: identity.invocation.clone(),
                root_kind: StreamRootKindV1::MethodResult,
                recursive_value_path: vec![StreamValuePathStepV1::TupleElement(1)],
            },
            StreamSourceKindV1::InvocationOutput,
        );
        second_request.element_schema_fingerprint =
            schema_fingerprint_v1(&source_graph, Some(&source_element_type)).unwrap();
        let second_handle = producer.register(second_request).await.unwrap().value;
        let streams = DurableSessionStreams::new(
            producer,
            oplog.clone(),
            identity.invocation,
            [
                (0, handle.clone(), SessionStreamRoleV1::Input),
                (1, second_handle.clone(), SessionStreamRoleV1::Input),
            ],
        )
        .with_consumer_journal(Arc::new(TestConsumerJournal(oplog)));
        let stream = SchemaValueStream::from_host_endpoint(ForwardedDurableInput {
            handle: handle.clone(),
        });
        let mismatched_element_type = SchemaType::string();
        let mismatched_graph = SchemaGraph::anonymous(mismatched_element_type.clone());

        let error = streams
            .materialize_result(
                SchemaValue::Stream(stream.clone()),
                &mismatched_graph,
                &SchemaType::stream(Some(mismatched_element_type.clone())),
                golem_common::model::component::ComponentRevision::INITIAL,
            )
            .await
            .unwrap_err();

        assert!(error.contains("does not match the result stream schema"));
        assert!(
            stream
                .with_host_endpoint::<ForwardedDurableInput, _>(|_| ())
                .is_ok()
        );

        let endpoint_stream = SchemaValueStream::from_host_endpoint(
            streams
                .endpoint(handle, 0, SessionStreamRoleV1::Input)
                .await
                .unwrap(),
        );
        let error = streams
            .materialize_result(
                SchemaValue::Stream(endpoint_stream.clone()),
                &mismatched_graph,
                &SchemaType::stream(Some(mismatched_element_type)),
                golem_common::model::component::ComponentRevision::INITIAL,
            )
            .await
            .unwrap_err();

        assert!(error.contains("does not match the result stream schema"));
        assert!(
            endpoint_stream
                .with_host_endpoint::<DurableInputEndpoint, _>(|_| ())
                .is_ok()
        );

        let second_stream = SchemaValueStream::from_host_endpoint(ForwardedDurableInput {
            handle: second_handle,
        });
        let tuple_root = SchemaType::tuple(vec![
            SchemaType::stream(Some(source_element_type.clone())),
            SchemaType::stream(Some(SchemaType::string())),
        ]);
        let tuple_graph = SchemaGraph::anonymous(tuple_root.clone());
        let error = streams
            .materialize_result(
                SchemaValue::Tuple {
                    elements: vec![
                        SchemaValue::Stream(endpoint_stream.clone()),
                        SchemaValue::Stream(second_stream.clone()),
                    ],
                },
                &tuple_graph,
                &tuple_root,
                golem_common::model::component::ComponentRevision::INITIAL,
            )
            .await
            .unwrap_err();

        assert!(error.contains("does not match the result stream schema"));
        assert!(
            endpoint_stream
                .with_host_endpoint::<DurableInputEndpoint, _>(|_| ())
                .is_ok()
        );
        assert!(
            second_stream
                .with_host_endpoint::<ForwardedDurableInput, _>(|_| ())
                .is_ok()
        );

        let consumed = SchemaValueStream::from_host_endpoint(());
        consumed.take_host_endpoint::<()>().unwrap();
        let valid_tuple_root = SchemaType::tuple(vec![
            SchemaType::stream(Some(source_element_type.clone())),
            SchemaType::stream(Some(source_element_type)),
        ]);
        let valid_tuple_graph = SchemaGraph::anonymous(valid_tuple_root.clone());

        let error = streams
            .materialize_result(
                SchemaValue::Tuple {
                    elements: vec![
                        SchemaValue::Stream(second_stream.clone()),
                        SchemaValue::Stream(consumed),
                    ],
                },
                &valid_tuple_graph,
                &valid_tuple_root,
                golem_common::model::component::ComponentRevision::INITIAL,
            )
            .await
            .unwrap_err();

        assert!(error.contains("schema value stream was already transferred"));
        assert!(
            second_stream
                .with_host_endpoint::<ForwardedDurableInput, _>(|_| ())
                .is_ok()
        );

        let error = streams
            .materialize_result(
                SchemaValue::Tuple {
                    elements: vec![
                        SchemaValue::Stream(second_stream.clone()),
                        SchemaValue::Stream(second_stream.clone()),
                    ],
                },
                &valid_tuple_graph,
                &valid_tuple_root,
                golem_common::model::component::ComponentRevision::INITIAL,
            )
            .await
            .unwrap_err();

        assert!(error.contains("the same affine stream appeared more than once"));
        assert!(
            second_stream
                .with_host_endpoint::<ForwardedDurableInput, _>(|_| ())
                .is_ok()
        );
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
