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
    StreamCancelRoleV1, StreamConsumerCancelAppliedRecordV1, StreamConsumerCancelIntentRecordV1,
    StreamConsumerItemValueRecordV1, StreamConsumerTerminalRecordV1, StreamConsumerTerminalV1,
    StreamEndResultV1, StreamInvocationIdV1, StreamItemsPayloadV1, StreamRegistrationCoordinateV1,
    StreamResumeOperationV1, StreamRootKindV1, StreamSessionCancelRequestedRecordV1,
    StreamSessionDetachedRecordV1, StreamSessionInvocationResultRecordV1, StreamSessionKeyV1,
    StreamSessionMappingRecordV1, StreamSessionMappingUpdateRecordV1, StreamSessionMappingV1,
    StreamSessionRecordV1, StreamSessionResumeAttemptRecordV1, StreamSlotTombstonedRecordV1,
    StreamSourceKindV1, StreamTopologyActivatedRecordV1, StreamTopologyPreparedRecordV1,
    StreamValuePathStepV1,
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
    applied_cancel_intents: HashSet<StreamConsumerCancelIntentRecordV1>,
    tombstoned_slots: HashMap<String, SessionStreamRoleV1>,
    cancellation_requested: bool,
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

    pub(crate) fn has_committed_cancellation(
        &self,
        key: &StreamAttachmentKeyV1,
        mapping: &StreamSessionMappingRecordV1,
        intent: &StreamConsumerCancelIntentRecordV1,
    ) -> Result<bool, String> {
        if self.malformed_record {
            return Err("unsupported or malformed durable Stream Session record version".into());
        }
        if let Some(error) = &self.topology_error {
            return Err(error.clone());
        }
        let consumer_matches = key.consumer_invocation == key.session_key
            || self.topologies.values().any(|topology| {
                let mut current_key = key.clone();
                current_key.epoch = topology.attachment.epoch;
                topology.attachment == current_key && topology.mapping == *mapping
            });
        Ok(consumer_matches
            && intent.session_key == key.session_key
            && intent.stream_id == key.stream_id
            && intent.epoch == key.epoch
            && mapping.handle.stream_id == key.stream_id
            && self.cancel_intents.get(&key.stream_id) == Some(intent)
            && self.persisted_mappings.contains(&(
                mapping.transport_stream_id,
                mapping.handle.clone(),
                mapping.role,
            )))
    }

    pub(crate) fn needs_recovery(
        &self,
        owner: &golem_common::model::OwnedAgentId,
        key: &StreamSessionKeyV1,
    ) -> bool {
        self.needs_topology_recovery(owner, key)
            || self
                .cancel_intents
                .values()
                .any(|intent| !self.applied_cancel_intents.contains(intent))
    }

    pub(crate) fn has_cancellation_intents(&self) -> bool {
        self.cancel_intents
            .values()
            .any(|intent| !self.applied_cancel_intents.contains(intent))
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
                        .cancel_intents
                        .contains_key(&topology.attachment.stream_id)
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
                        .cancel_intents
                        .contains_key(&topology.attachment.stream_id)
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
        if let StreamSessionRecordV1::ConsumerCancelApplied(record) = record
            && &record.intent.session_key == key
            && self.cancel_intents.get(&record.intent.stream_id) == Some(&record.intent)
        {
            self.applied_cancel_intents.insert(record.intent.clone());
        }
        if let StreamSessionRecordV1::Tombstoned(record) = record
            && &record.session_key == key
        {
            self.tombstoned_slots
                .entry(record.slot.clone())
                .or_insert(record.role);
        }
        if let StreamSessionRecordV1::CancelRequested(record) = record
            && &record.session_key == key
        {
            self.cancellation_requested = true;
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
                        if let StreamRegistrationCoordinateV1::Nested {
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
            .write_attached_items_with_nested(
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
            .append_external_input(
                &self.session_key,
                handle.stream_id,
                None,
                true,
                Some(super::durable_stream::ExternalProducerV1 {
                    id: golem_common::base_model::durable_stream::ExternalProducerIdV1::Attached,
                    epoch: 0,
                    sequence,
                }),
            )
            .await
        {
            Ok(
                super::durable_stream::ExternalAppendOutcomeV1::Accepted(offset)
                | super::durable_stream::ExternalAppendOutcomeV1::Duplicate { offset, .. },
            ) => Ok(Some(offset)),
            Ok(super::durable_stream::ExternalAppendOutcomeV1::Closed) => Ok(None),
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

    pub(crate) async fn cancel_session_streams(&self) -> Result<bool, String> {
        if !self.has_local_session_authority() {
            return Err("session cancellation requires the session owner".into());
        }
        let session = self.clone();
        let exists = self
            .producer
            .run_lifecycle(0, move |owner| async move {
                let _guard = session.session_lock.lock().await;
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
                    records.push(StreamSessionRecordV1::CancelRequested(
                        StreamSessionCancelRequestedRecordV1 {
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
                        records.push(StreamSessionRecordV1::ConsumerCancelIntent(
                            StreamConsumerCancelIntentRecordV1 {
                                format_version: DURABLE_STREAM_FORMAT_VERSION,
                                session_key: session.session_key.clone(),
                                stream_id: handle.stream_id,
                                epoch,
                                role: match role {
                                    SessionStreamRoleV1::Input => StreamCancelRoleV1::InputProducer,
                                    SessionStreamRoleV1::Output => {
                                        StreamCancelRoleV1::OutputConsumer
                                    }
                                },
                                reason: StreamCancelReasonV1::Cancelled,
                                details: None,
                            },
                        ));
                    }
                }
                drop(metadata);
                if !records.is_empty() {
                    owner
                        .append_session_records_owned(session.entity_parent_start_index, records)
                        .await
                        .map_err(|error| error.to_string())?;
                }
                Ok(true)
            })
            .await?;
        if exists {
            self.reconcile_local_cancellation_intents().await?;
        }
        Ok(exists)
    }

    /// The caller resolves the canonical slot under this session guard inside an owned lifecycle operation.
    pub(crate) async fn tombstone_slot_owned(
        &self,
        slot: String,
        stream: Option<(DurableStreamHandleV1, SessionStreamRoleV1)>,
        session_guard: tokio::sync::MutexGuard<'_, ()>,
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
        let mut records = vec![StreamSessionRecordV1::Tombstoned(
            StreamSlotTombstonedRecordV1 {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: self.session_key.clone(),
                slot,
                role: stream
                    .as_ref()
                    .map_or(SessionStreamRoleV1::Output, |(_, role)| *role),
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
                records.push(StreamSessionRecordV1::ConsumerCancelIntent(
                    StreamConsumerCancelIntentRecordV1 {
                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                        session_key: self.session_key.clone(),
                        stream_id: handle.stream_id,
                        epoch,
                        role: match role {
                            SessionStreamRoleV1::Input => StreamCancelRoleV1::InputProducer,
                            SessionStreamRoleV1::Output => StreamCancelRoleV1::OutputConsumer,
                        },
                        reason: StreamCancelReasonV1::Cancelled,
                        details: None,
                    },
                ));
            }
        }
        drop(metadata);
        self.producer
            .append_session_records_owned(self.entity_parent_start_index, records)
            .await
            .map_err(|error| error.to_string())?;
        drop(session_guard);
        self.reconcile_local_cancellation_intents().await?;
        Ok(true)
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
        self.apply_cancel_intent_owned(mapping, intent, session_guard)
            .await
    }

    pub(crate) async fn reconcile_local_cancellation_intents(&self) -> Result<(), String> {
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
                    StreamSessionMappingRecordV1 {
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
                .run_lifecycle(
                    intent.details.as_ref().map_or(0, String::len),
                    move |_| async move {
                        let _guard = session.session_lock.lock().await;
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
                                session.entity_parent_start_index,
                                StreamSessionRecordV1::ConsumerCancelApplied(
                                    StreamConsumerCancelAppliedRecordV1 {
                                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                                        intent,
                                    },
                                ),
                            )
                            .await
                            .map_err(|error| error.to_string())?;
                        Ok::<_, String>(())
                    },
                )
                .await?;
        }
        Ok(())
    }

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
            let mapping = StreamSessionMappingRecordV1 {
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
            let result = self
                .producer
                .run_lifecycle(
                    intent.details.as_ref().map_or(0, String::len),
                    move |owner| async move {
                        owner.defer_remote_cancellation(async move {
                            tokio::time::timeout(
                                timeout,
                                RoutedStreamAttachmentControl::new(rpc, mapping, auth_ctx)
                                    .cancel_stream(key, intent.role, intent.reason, intent.details),
                            )
                            .await
                            .map_err(|_| DurableStreamProducerError::RecoveryRequired)?
                            .map(|_| ())
                        });
                        Ok::<_, String>(())
                    },
                )
                .await;
            if let Err(error) = result {
                first_error.get_or_insert(error);
            } else {
                // The routed operation has released its lifecycle admission before the
                // acknowledgment takes a new one. A lost acknowledgment safely retries.
                let attribution = self.entity_parent_start_index;
                let receipt = self
                    .producer
                    .run_lifecycle(0, move |owner| async move {
                        owner
                            .append_session_record_owned(
                                attribution,
                                StreamSessionRecordV1::ConsumerCancelApplied(
                                    StreamConsumerCancelAppliedRecordV1 {
                                        format_version: DURABLE_STREAM_FORMAT_VERSION,
                                        intent: receipt_intent,
                                    },
                                ),
                            )
                            .await
                            .map_err(|error| error.to_string())
                    })
                    .await;
                if let Err(error) = receipt {
                    first_error.get_or_insert(error);
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    async fn apply_cancel_intent_owned(
        &self,
        mapping: StreamSessionMappingRecordV1,
        intent: StreamConsumerCancelIntentRecordV1,
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
            let pending = self
                .producer
                .commit_cancel_open(
                    mapping.handle.stream_id,
                    intent.role,
                    intent.reason,
                    intent.details.clone(),
                )
                .await
                .map_err(|error| error.to_string())?;
            self.producer
                .append_session_record_owned(
                    self.entity_parent_start_index,
                    StreamSessionRecordV1::ConsumerCancelApplied(
                        StreamConsumerCancelAppliedRecordV1 {
                            format_version: DURABLE_STREAM_FORMAT_VERSION,
                            intent: intent.clone(),
                        },
                    ),
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
                .attached_input_high_water(&self.session_key, handle.stream_id)
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
        let metadata = self.current_control_metadata().await?;
        let first_result = metadata.invocation_result.is_none();
        let cancel_session = metadata.cancellation_requested;
        let deleted_outputs = metadata
            .tombstoned_slots
            .iter()
            .filter(|(_, role)| **role == SessionStreamRoleV1::Output)
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
            path: Vec<StreamValuePathStepV1>,
            endpoint: Option<LiveStreamEndpoint>,
            forwarded_handle: Option<DurableStreamHandleV1>,
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
                    [StreamValuePathStepV1::RecordField(index)] => {
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
                if pending.cancelled || existing_intents.contains(&handle.stream_id) {
                    self.mapping_for_handle(handle, SessionStreamRoleV1::Output)
                        .map(|mapping| Ok(mapping.transport_stream_id))
                        .unwrap_or_else(|| self.allocate_transport_stream_id())?
                } else {
                    self.ensure_nested_mapping_under_lock(
                        handle.clone(),
                        SessionStreamRoleV1::Output,
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
                    cancellation_epoch: cancellation_epoch.filter(|_| {
                        pending.cancelled
                            && !pending
                                .forwarded_handle
                                .as_ref()
                                .is_some_and(|handle| existing_intents.contains(&handle.stream_id))
                    }),
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
            if let Some(endpoint) = pending.endpoint
                && !pending.cancelled
            {
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
        DurableStreamProducerError::ClosedByOtherProducer
            | DurableStreamProducerError::FencedByTerminal(
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
mod tests;
