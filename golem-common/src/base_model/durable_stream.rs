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

use crate::base_model::component::ComponentRevision;
use crate::base_model::environment::EnvironmentId;
use crate::base_model::{AgentFingerprint, AgentId, IdempotencyKey, OplogIndex};
use golem_schema::schema::{
    FromSchema as FromSchemaTrait, FromSchemaError, IntoSchema as IntoSchemaTrait, SchemaBuilder,
    SchemaFingerprintV1, SchemaType, SchemaValue, TypeId,
};
use golem_schema_derive::{FromSchema, IntoSchema};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt::{Display, Formatter};
use uuid::Uuid;

pub const DURABLE_STREAM_FORMAT_VERSION: u8 = 1;
pub const STREAM_ID_NAMESPACE_V1: Uuid = Uuid::from_u128(0x7125b775_3cb6_58b6_82c8_e7584ae91b2a);
pub const ATTACHMENT_ID_NAMESPACE_V1: Uuid =
    Uuid::from_u128(0xde596c85_8f1b_55ab_9430_63aced239572);

pub const MAX_DURABLE_STREAM_ITEM_SIZE: usize = 16 * 1024 * 1024;
pub const MAX_PACKED_U8_STREAM_ITEM_SIZE: usize = 1024 * 1024;
pub const MAX_DURABLE_STREAMS_PER_SESSION: usize = 1024;
pub const MAX_NEW_STREAM_HANDLES_PER_VALUE: usize = 256;
pub const MAX_STREAM_VALUE_TRAVERSAL_DEPTH: usize = 128;
pub const MAX_LIVE_READERS_PER_STREAM: usize = 16;
pub const DEFAULT_LIVE_JOIN_BUFFER_SIZE: usize = 32;
pub const MIN_LIVE_JOIN_BUFFER_SIZE: usize = 1;
pub const MAX_LIVE_JOIN_BUFFER_SIZE: usize = 1024;
pub const STREAM_ATTACHMENT_LEASE_TTL_MILLIS: u64 = 60_000;
pub const STREAM_ATTACHMENT_RENEWAL_TARGET_MILLIS: u64 = 20_000;
pub const STREAM_ATTACHMENT_RECONCILIATION_INTERVAL_MILLIS: u64 = 30_000;
pub const STREAM_ATTACHMENT_RECONCILIATION_BATCH_SIZE: usize = 256;
pub const STREAM_ATTACHMENT_ABANDONED_PREPARE_MILLIS: u64 = 5 * 60_000;

#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    Serialize,
    Deserialize,
    IntoSchema,
    FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
#[serde(transparent)]
#[schema(transparent)]
pub struct StreamId(pub Uuid);

impl StreamId {
    pub fn derive(
        environment_id: EnvironmentId,
        producer: &AgentId,
        expected_producer_fingerprint: AgentFingerprint,
        registration_oplog_index: OplogIndex,
    ) -> Result<Self, DurableStreamIdentityError> {
        let agent_name = producer.agent_id.as_bytes();
        let name_length: u32 = agent_name
            .len()
            .try_into()
            .map_err(|_| DurableStreamIdentityError::AgentNameTooLong)?;
        let mut name = Vec::with_capacity(53 + agent_name.len());
        name.push(DURABLE_STREAM_FORMAT_VERSION);
        name.extend_from_slice(environment_id.0.as_bytes());
        name.extend_from_slice(producer.component_id.0.as_bytes());
        name.extend_from_slice(&name_length.to_be_bytes());
        name.extend_from_slice(agent_name);
        name.extend_from_slice(expected_producer_fingerprint.0.as_bytes());
        name.extend_from_slice(&registration_oplog_index.as_u64().to_be_bytes());
        Ok(Self(Uuid::new_v5(&STREAM_ID_NAMESPACE_V1, &name)))
    }
}

impl Display for StreamId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.0, f)
    }
}

#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    Serialize,
    Deserialize,
    IntoSchema,
    FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
#[serde(transparent)]
#[schema(transparent)]
pub struct AttachmentId(pub Uuid);

impl AttachmentId {
    pub fn primary(
        callee_environment_id: EnvironmentId,
        callee: &AgentId,
        invocation_key: &IdempotencyKey,
    ) -> Result<Self, DurableStreamIdentityError> {
        let agent_name = callee.agent_id.as_bytes();
        let agent_name_length: u32 = agent_name
            .len()
            .try_into()
            .map_err(|_| DurableStreamIdentityError::AgentNameTooLong)?;
        let invocation_key = invocation_key.value.as_bytes();
        let invocation_key_length: u32 = invocation_key
            .len()
            .try_into()
            .map_err(|_| DurableStreamIdentityError::InvocationKeyTooLong)?;
        let mut name = Vec::with_capacity(45 + agent_name.len() + invocation_key.len());
        name.push(DURABLE_STREAM_FORMAT_VERSION);
        name.extend_from_slice(callee_environment_id.0.as_bytes());
        name.extend_from_slice(callee.component_id.0.as_bytes());
        name.extend_from_slice(&agent_name_length.to_be_bytes());
        name.extend_from_slice(agent_name);
        name.extend_from_slice(&invocation_key_length.to_be_bytes());
        name.extend_from_slice(invocation_key);
        name.extend_from_slice(&0u32.to_be_bytes());
        Ok(Self(Uuid::new_v5(&ATTACHMENT_ID_NAMESPACE_V1, &name)))
    }
}

impl Display for AttachmentId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.0, f)
    }
}

#[derive(
    Clone,
    Copy,
    Debug,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    Serialize,
    Deserialize,
    IntoSchema,
    FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
#[serde(transparent)]
#[schema(transparent)]
pub struct AttemptId(pub Uuid);

impl AttemptId {
    pub fn fresh() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Display for AttemptId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.0, f)
    }
}

/// Opaque v1 stream position. Raw-byte ordering is the protocol ordering.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
#[serde(transparent)]
pub struct StreamOffsetV1(pub [u8; 24]);

impl IntoSchemaTrait for StreamOffsetV1 {
    fn type_id() -> TypeId {
        TypeId::new("golem_common.base_model.StreamOffsetV1")
    }

    fn register_in(builder: &mut SchemaBuilder) -> SchemaType {
        <Vec<u8> as IntoSchemaTrait>::register_in(builder)
    }

    fn to_value(&self) -> SchemaValue {
        self.0.to_vec().to_value()
    }
}

impl FromSchemaTrait for StreamOffsetV1 {
    fn from_value(value: &SchemaValue) -> Result<Self, FromSchemaError> {
        let bytes = Vec::<u8>::from_value(value)?;
        let bytes: [u8; 24] = bytes.try_into().map_err(|bytes: Vec<u8>| {
            FromSchemaError::custom(format!(
                "stream offset must have 24 bytes, got {}",
                bytes.len()
            ))
        })?;
        Self::from_bytes(bytes).map_err(|error| FromSchemaError::custom(error.to_string()))
    }
}

impl StreamOffsetV1 {
    pub const FORMAT_VERSION: u8 = 1;

    pub fn new(producer_oplog_index: OplogIndex, sub_index: u32) -> Self {
        let mut bytes = [0u8; 24];
        bytes[0] = Self::FORMAT_VERSION;
        bytes[8..16].copy_from_slice(&producer_oplog_index.as_u64().to_be_bytes());
        bytes[16..20].copy_from_slice(&sub_index.to_be_bytes());
        Self(bytes)
    }

    pub fn from_bytes(bytes: [u8; 24]) -> Result<Self, StreamOffsetError> {
        if bytes[0] != Self::FORMAT_VERSION {
            return Err(StreamOffsetError::UnsupportedVersion(bytes[0]));
        }
        if bytes[1..8].iter().any(|byte| *byte != 0) || bytes[20..24].iter().any(|byte| *byte != 0)
        {
            return Err(StreamOffsetError::ReservedBitsSet);
        }
        Ok(Self(bytes))
    }

    pub fn as_bytes(&self) -> &[u8; 24] {
        &self.0
    }

    pub fn producer_oplog_index(self) -> OplogIndex {
        OplogIndex::from_u64(u64::from_be_bytes(
            self.0[8..16]
                .try_into()
                .expect("stream offset oplog index has fixed width"),
        ))
    }

    pub fn sub_index(self) -> u32 {
        u32::from_be_bytes(
            self.0[16..20]
                .try_into()
                .expect("stream offset sub-index has fixed width"),
        )
    }
}

impl Display for StreamOffsetV1 {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum StreamOffsetError {
    #[error("unsupported stream offset format version {0}")]
    UnsupportedVersion(u8),
    #[error("stream offset reserved bits are set")]
    ReservedBitsSet,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum DurableStreamIdentityError {
    #[error("agent name is too long for a durable stream identity")]
    AgentNameTooLong,
    #[error("idempotency key is too long for a durable stream identity")]
    InvocationKeyTooLong,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
pub struct StreamInvocationIdV1 {
    pub callee_environment_id: EnvironmentId,
    pub callee: AgentId,
    pub callee_fingerprint: AgentFingerprint,
    pub idempotency_key: IdempotencyKey,
}

pub type StreamSessionKeyV1 = StreamInvocationIdV1;

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
pub struct DurableStreamHandleV1 {
    pub format_version: u8,
    pub stream_id: StreamId,
    pub producer_environment_id: EnvironmentId,
    pub producer: AgentId,
    pub expected_producer_fingerprint: AgentFingerprint,
    pub source_invocation: StreamInvocationIdV1,
    pub component_revision: ComponentRevision,
    pub element_schema_fingerprint: SchemaFingerprintV1,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
pub enum StreamRegistrationCoordinateV1 {
    Root {
        invocation_id: StreamInvocationIdV1,
        root_kind: StreamRootKindV1,
        recursive_value_path: Vec<StreamValuePathStepV1>,
    },
    Nested {
        parent_stream_id: StreamId,
        parent_producer_sequence: u64,
        recursive_value_path: Vec<StreamValuePathStepV1>,
    },
}

#[derive(
    Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamRootKindV1 {
    MethodInput,
    MethodResult,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum StreamValuePathStepV1 {
    RecordField(u32),
    VariantCasePayload(u32),
    TupleElement(u32),
    ListElement(u32),
    FixedListElement(u32),
    MapEntry { index: u32, side: StreamMapSideV1 },
    OptionSome,
    ResultOk,
    ResultErr,
    UnionBranch(u32),
}

#[derive(
    Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamMapSideV1 {
    Key,
    Value,
}

#[derive(
    Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamSourceKindV1 {
    ExternalInlineInput,
    AgentHostedInput,
    InvocationOutput,
    Nested,
    Forwarded,
}

#[derive(
    Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum SessionStreamRoleV1 {
    Input,
    Output,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
pub struct StreamSessionMappingV1 {
    pub session_key: StreamSessionKeyV1,
    pub attachment_id: AttachmentId,
    pub role: SessionStreamRoleV1,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamRegisteredRecordV1 {
    pub format_version: u8,
    pub coordinate: StreamRegistrationCoordinateV1,
    pub registration_oplog_index: OplogIndex,
    pub handle: DurableStreamHandleV1,
    pub source_kind: StreamSourceKindV1,
    pub session_mapping: Option<StreamSessionMappingV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamItemsRecordV1 {
    pub format_version: u8,
    pub stream_id: StreamId,
    pub producer_fingerprint: AgentFingerprint,
    pub first_sequence: u64,
    pub nested_stream_ids: Vec<StreamId>,
    pub newly_registered_stream_ids: Vec<StreamId>,
    pub payload: StreamItemsPayloadV1,
    pub offsets: Vec<StreamOffsetV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamItemsPayloadV1 {
    /// Canonically encoded complete durable values. V1 normally stores one value per entry.
    Values(Vec<Vec<u8>>),
    PackedU8(Vec<u8>),
}

impl StreamItemsPayloadV1 {
    pub fn logical_item_count(&self) -> usize {
        match self {
            Self::Values(values) => values.len(),
            Self::PackedU8(bytes) => bytes.len(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamEndRecordV1 {
    pub format_version: u8,
    pub stream_id: StreamId,
    pub producer_fingerprint: AgentFingerprint,
    pub sequence: u64,
    pub offset: StreamOffsetV1,
    pub authored_by: StreamTerminalAuthorV1,
    pub result: StreamEndResultV1,
}

#[derive(
    Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamTerminalAuthorV1 {
    Guest,
    Protocol,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamEndResultV1 {
    Ok,
    ErrorContext(Vec<u8>),
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamCancelRecordV1 {
    pub format_version: u8,
    pub stream_id: StreamId,
    pub producer_fingerprint: AgentFingerprint,
    pub sequence: u64,
    pub offset: StreamOffsetV1,
    pub authored_by: StreamTerminalAuthorV1,
    pub role: StreamCancelRoleV1,
    pub reason: StreamCancelReasonV1,
    pub details: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamCancelRoleV1 {
    InputProducer,
    InputConsumer,
    OutputProducer,
    OutputConsumer,
    System,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamCancelReasonV1 {
    Cancelled,
    GuestDrop,
    Protocol,
    InvocationFailed,
    SourceUnavailable,
    ProducerDeleting,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct PersistedStreamInvocationDescriptorV1 {
    pub format_version: u8,
    pub session_key: StreamSessionKeyV1,
    pub target_component_revision: ComponentRevision,
    pub method_name: String,
    /// Canonical serialization of the complete recursive invocation value after replacing each
    /// stream leaf with its corresponding durable handle in `stream_handles`.
    pub invocation_value: Vec<u8>,
    pub stream_handles: Vec<DurableStreamHandleV1>,
    /// Canonical execution-mode and configuration bytes that affect the call.
    pub execution_config: Vec<u8>,
    /// Canonical effective principal and grant identity. Credential bytes and expiry are excluded.
    pub effective_identity: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StartAttemptDescriptorV1 {
    pub format_version: u8,
    pub session_key: StreamSessionKeyV1,
    pub attachment_id: AttachmentId,
    pub expected_callee_fingerprint: AgentFingerprint,
    pub attempt_id: AttemptId,
    pub invocation: PersistedStreamInvocationDescriptorV1,
    pub effective_identity: Vec<u8>,
    pub live_join_buffer_events: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamSessionPreparedRecordV1 {
    pub format_version: u8,
    pub attempt: StartAttemptDescriptorV1,
    pub stream_mappings: Vec<StreamSessionMappingRecordV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamSessionMappingRecordV1 {
    /// Transport-local source reference used only to route frames within this attachment.
    pub transport_stream_id: u64,
    pub handle: DurableStreamHandleV1,
    pub role: SessionStreamRoleV1,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamSessionMappingUpdateRecordV1 {
    pub format_version: u8,
    pub session_key: StreamSessionKeyV1,
    pub mapping: StreamSessionMappingRecordV1,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamAttachmentKeyV1 {
    pub attachment_id: AttachmentId,
    pub stream_id: StreamId,
    pub epoch: u64,
    pub session_key: StreamSessionKeyV1,
    pub producer_environment_id: EnvironmentId,
    pub producer: AgentId,
    pub expected_producer_fingerprint: AgentFingerprint,
    pub consumer_environment_id: EnvironmentId,
    pub consumer: AgentId,
    pub expected_consumer_fingerprint: AgentFingerprint,
    pub consumer_invocation: StreamInvocationIdV1,
}

impl StreamAttachmentKeyV1 {
    fn is_well_formed(&self) -> bool {
        self.epoch > 0
            && self.consumer_invocation.callee_environment_id == self.consumer_environment_id
            && self.consumer_invocation.callee == self.consumer
            && self.consumer_invocation.callee_fingerprint == self.expected_consumer_fingerprint
            && AttachmentId::primary(
                self.session_key.callee_environment_id,
                &self.session_key.callee,
                &self.session_key.idempotency_key,
            )
            .is_ok_and(|attachment_id| attachment_id == self.attachment_id)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamAttachmentPreparedRecordV1 {
    pub format_version: u8,
    pub key: StreamAttachmentKeyV1,
    pub prepared_at_millis: u64,
    pub lease_expires_at_millis: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamAttachmentActivatedRecordV1 {
    pub format_version: u8,
    pub key: StreamAttachmentKeyV1,
    pub activated_at_millis: u64,
    pub lease_expires_at_millis: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamAttachmentRenewedRecordV1 {
    pub format_version: u8,
    pub key: StreamAttachmentKeyV1,
    pub renewed_at_millis: u64,
    pub lease_expires_at_millis: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamAttachmentFinalizationReasonV1 {
    ConsumerFinalized,
    ConsumerDeleted,
    ConsumerIncarnationChanged,
    PrepareAbandoned,
    Reconciled,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamAttachmentControlRequestV1 {
    pub format_version: u8,
    pub mapping: Option<StreamSessionMappingRecordV1>,
    pub operation: StreamAttachmentControlOperationV1,
}

impl StreamAttachmentControlRequestV1 {
    pub fn is_well_formed(&self) -> bool {
        self.format_version == DURABLE_STREAM_FORMAT_VERSION
            && self.operation.key().is_well_formed()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(
    feature = "full",
    desert(evolution(FieldAdded("wait_for_events", false)))
)]
pub struct AttachedStreamSegmentRequestV1 {
    pub format_version: u8,
    pub attachment: StreamAttachmentKeyV1,
    pub mapping: StreamSessionMappingRecordV1,
    pub after: Option<StreamOffsetV1>,
    pub through: Option<StreamOffsetV1>,
    pub wait_for_events: bool,
}

impl AttachedStreamSegmentRequestV1 {
    pub fn is_well_formed(&self) -> bool {
        self.format_version == DURABLE_STREAM_FORMAT_VERSION
            && self.attachment.is_well_formed()
            && topology_mapping_matches(&self.attachment, &self.mapping)
            && (!self.wait_for_events || self.through.is_none())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamAttachmentControlOperationV1 {
    Prepare {
        key: StreamAttachmentKeyV1,
        now_millis: u64,
    },
    Activate {
        key: StreamAttachmentKeyV1,
        now_millis: u64,
    },
    Detach {
        key: StreamAttachmentKeyV1,
    },
    Renew {
        key: StreamAttachmentKeyV1,
        now_millis: u64,
    },
    Cancel {
        key: StreamAttachmentKeyV1,
        role: StreamCancelRoleV1,
        reason: StreamCancelReasonV1,
        details: Option<String>,
    },
    Finalize {
        key: StreamAttachmentKeyV1,
        reason: StreamAttachmentFinalizationReasonV1,
        now_millis: u64,
    },
    SourceUnavailable {
        key: StreamAttachmentKeyV1,
        source_offset: StreamOffsetV1,
        consumer_read_ordinal: u64,
    },
}

impl StreamAttachmentControlOperationV1 {
    pub fn key(&self) -> &StreamAttachmentKeyV1 {
        match self {
            Self::Prepare { key, .. }
            | Self::Activate { key, .. }
            | Self::Detach { key }
            | Self::Renew { key, .. }
            | Self::Cancel { key, .. }
            | Self::Finalize { key, .. }
            | Self::SourceUnavailable { key, .. } => key,
        }
    }

    pub fn targets_consumer(&self) -> bool {
        matches!(self, Self::SourceUnavailable { .. })
    }
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamAttachmentFinalizedRecordV1 {
    pub format_version: u8,
    pub key: StreamAttachmentKeyV1,
    pub finalized_at_millis: u64,
    pub reason: StreamAttachmentFinalizationReasonV1,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamProducerDeletingRecordV1 {
    pub format_version: u8,
    pub producer_environment_id: EnvironmentId,
    pub producer: AgentId,
    pub producer_fingerprint: AgentFingerprint,
    pub deleting_at_millis: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamCascadeDependentResultV1 {
    ConsumerJournalComplete,
    SourceUnavailable {
        first_unjournaled_offset: StreamOffsetV1,
    },
    ConsumerDeleted,
    ConsumerIncarnationChanged,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamCascadeOutboxRecordV1 {
    pub format_version: u8,
    pub key: StreamAttachmentKeyV1,
    pub completed_at_millis: u64,
    pub result: StreamCascadeDependentResultV1,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamConsumerDeletingRecordV1 {
    pub format_version: u8,
    pub consumer_environment_id: EnvironmentId,
    pub consumer: AgentId,
    pub consumer_fingerprint: AgentFingerprint,
    pub deleting_at_millis: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamSourceUnavailableRecordV1 {
    pub format_version: u8,
    pub key: StreamAttachmentKeyV1,
    pub source_offset: StreamOffsetV1,
    pub consumer_read_ordinal: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamTopologyPreparedRecordV1 {
    pub format_version: u8,
    pub session_key: StreamSessionKeyV1,
    pub attachment: StreamAttachmentKeyV1,
    pub mapping: StreamSessionMappingRecordV1,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamTopologyActivatedRecordV1 {
    pub format_version: u8,
    pub session_key: StreamSessionKeyV1,
    pub attachment: StreamAttachmentKeyV1,
    pub mapping: StreamSessionMappingRecordV1,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamSessionAttachedRecordV1 {
    pub format_version: u8,
    pub session_key: StreamSessionKeyV1,
    pub attachment_id: AttachmentId,
    pub attempt_id: AttemptId,
    pub epoch: u64,
    pub pending_invocation_oplog_index: OplogIndex,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamResumeOperationV1 {
    Resume,
    Takeover,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamResumeCursorV1 {
    pub stream_id: StreamId,
    pub last_observed_offset: Option<StreamOffsetV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct ResumeAttemptDescriptorV1 {
    pub format_version: u8,
    pub operation: StreamResumeOperationV1,
    pub session_key: StreamSessionKeyV1,
    pub attachment_id: AttachmentId,
    pub expected_callee_fingerprint: AgentFingerprint,
    pub attempt_id: AttemptId,
    pub expected_epoch: u64,
    pub effective_identity: Vec<u8>,
    pub cursors: Vec<StreamResumeCursorV1>,
    pub live_join_buffer_events: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamSessionResumeAttemptRecordV1 {
    pub format_version: u8,
    pub attempt: ResumeAttemptDescriptorV1,
    pub accepted_epoch: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamSessionDetachedRecordV1 {
    pub format_version: u8,
    pub session_key: StreamSessionKeyV1,
    pub attachment_id: AttachmentId,
    pub owner_attempt_id: AttemptId,
    pub epoch: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct InputStreamHighWaterV1 {
    pub highest_contiguous_sequence: u64,
    pub resulting_offset: StreamOffsetV1,
    pub terminal: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamSessionInputHighWaterRecordV1 {
    pub format_version: u8,
    pub session_key: StreamSessionKeyV1,
    pub stream_id: StreamId,
    pub epoch: u64,
    pub first_sequence: u64,
    pub payload: StreamItemsPayloadV1,
    pub high_water: InputStreamHighWaterV1,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamConsumerItemValueRecordV1 {
    pub format_version: u8,
    pub session_key: StreamSessionKeyV1,
    pub stream_id: StreamId,
    pub source_offset: StreamOffsetV1,
    pub consumer_read_ordinal: u64,
    pub value: Vec<u8>,
    pub packed_u8: bool,
    pub recursive_handles: Vec<DurableStreamHandleV1>,
    pub recursive_mappings: Vec<StreamSessionMappingRecordV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamConsumerTerminalRecordV1 {
    pub format_version: u8,
    pub session_key: StreamSessionKeyV1,
    pub stream_id: StreamId,
    pub source_offset: StreamOffsetV1,
    pub consumer_read_ordinal: u64,
    pub terminal: StreamConsumerTerminalV1,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamConsumerCancelIntentRecordV1 {
    pub format_version: u8,
    pub session_key: StreamSessionKeyV1,
    pub stream_id: StreamId,
    pub epoch: u64,
    pub role: StreamCancelRoleV1,
    pub reason: StreamCancelReasonV1,
    pub details: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamConsumerTerminalV1 {
    End(StreamEndResultV1),
    Cancel {
        role: StreamCancelRoleV1,
        reason: StreamCancelReasonV1,
        details: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamSessionInvocationResultRecordV1 {
    pub format_version: u8,
    pub session_key: StreamSessionKeyV1,
    pub result: Vec<u8>,
    pub output_streams: Vec<DurableStreamHandleV1>,
    pub stream_mappings: Vec<StreamSessionMappingRecordV1>,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamSessionFinishedRecordV1 {
    pub format_version: u8,
    pub session_key: StreamSessionKeyV1,
    pub result: Result<(), Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamCallerAttemptRecordV1 {
    pub format_version: u8,
    pub session_key: StreamSessionKeyV1,
    pub attempt_id: AttemptId,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamSessionRecordV1 {
    CallerAttempt(StreamCallerAttemptRecordV1),
    Prepared(StreamSessionPreparedRecordV1),
    Attached(StreamSessionAttachedRecordV1),
    ResumeAttempt(StreamSessionResumeAttemptRecordV1),
    Detached(StreamSessionDetachedRecordV1),
    Mapping(StreamSessionMappingUpdateRecordV1),
    AttachmentPrepared(StreamAttachmentPreparedRecordV1),
    AttachmentActivated(StreamAttachmentActivatedRecordV1),
    AttachmentRenewed(StreamAttachmentRenewedRecordV1),
    AttachmentFinalized(StreamAttachmentFinalizedRecordV1),
    ProducerDeleting(StreamProducerDeletingRecordV1),
    CascadeOutbox(StreamCascadeOutboxRecordV1),
    ConsumerDeleting(StreamConsumerDeletingRecordV1),
    SourceUnavailable(StreamSourceUnavailableRecordV1),
    TopologyPrepared(StreamTopologyPreparedRecordV1),
    TopologyActivated(StreamTopologyActivatedRecordV1),
    InputHighWater(StreamSessionInputHighWaterRecordV1),
    ConsumerItemValue(StreamConsumerItemValueRecordV1),
    ConsumerCancelIntent(StreamConsumerCancelIntentRecordV1),
    ConsumerTerminal(StreamConsumerTerminalRecordV1),
    InvocationResult(StreamSessionInvocationResultRecordV1),
    Finished(StreamSessionFinishedRecordV1),
}

impl StreamSessionRecordV1 {
    pub fn format_version(&self) -> u8 {
        match self {
            Self::CallerAttempt(record) => record.format_version,
            Self::Prepared(record) => record.format_version,
            Self::Attached(record) => record.format_version,
            Self::ResumeAttempt(record) => record.format_version,
            Self::Detached(record) => record.format_version,
            Self::Mapping(record) => record.format_version,
            Self::AttachmentPrepared(record) => record.format_version,
            Self::AttachmentActivated(record) => record.format_version,
            Self::AttachmentRenewed(record) => record.format_version,
            Self::AttachmentFinalized(record) => record.format_version,
            Self::ProducerDeleting(record) => record.format_version,
            Self::CascadeOutbox(record) => record.format_version,
            Self::ConsumerDeleting(record) => record.format_version,
            Self::SourceUnavailable(record) => record.format_version,
            Self::TopologyPrepared(record) => record.format_version,
            Self::TopologyActivated(record) => record.format_version,
            Self::InputHighWater(record) => record.format_version,
            Self::ConsumerItemValue(record) => record.format_version,
            Self::ConsumerCancelIntent(record) => record.format_version,
            Self::ConsumerTerminal(record) => record.format_version,
            Self::InvocationResult(record) => record.format_version,
            Self::Finished(record) => record.format_version,
        }
    }

    pub fn has_supported_format(&self) -> bool {
        fn supported_handle(handle: &DurableStreamHandleV1) -> bool {
            handle.format_version == DURABLE_STREAM_FORMAT_VERSION
        }

        fn supported_attempt(attempt: &StartAttemptDescriptorV1) -> bool {
            attempt.format_version == DURABLE_STREAM_FORMAT_VERSION
                && attempt.attempt_id.0.get_version() == Some(uuid::Version::Random)
                && !attempt.attempt_id.0.is_nil()
                && attempt.expected_callee_fingerprint == attempt.session_key.callee_fingerprint
                && attempt.invocation.format_version == DURABLE_STREAM_FORMAT_VERSION
                && attempt.invocation.session_key == attempt.session_key
                && attempt.invocation.effective_identity == attempt.effective_identity
                && usize::try_from(attempt.live_join_buffer_events).is_ok_and(|capacity| {
                    (MIN_LIVE_JOIN_BUFFER_SIZE..=MAX_LIVE_JOIN_BUFFER_SIZE).contains(&capacity)
                })
                && AttachmentId::primary(
                    attempt.session_key.callee_environment_id,
                    &attempt.session_key.callee,
                    &attempt.session_key.idempotency_key,
                )
                .is_ok_and(|attachment_id| attachment_id == attempt.attachment_id)
        }

        if self.format_version() != DURABLE_STREAM_FORMAT_VERSION {
            return false;
        }
        match self {
            Self::Prepared(record) => {
                let unique_transport_ids = record
                    .stream_mappings
                    .iter()
                    .map(|mapping| mapping.transport_stream_id)
                    .collect::<HashSet<_>>();
                let unique_mappings = record
                    .stream_mappings
                    .iter()
                    .map(|mapping| (mapping.handle.clone(), mapping.role))
                    .collect::<HashSet<_>>();
                supported_attempt(&record.attempt)
                    && record
                        .attempt
                        .invocation
                        .stream_handles
                        .iter()
                        .all(supported_handle)
                    && record.stream_mappings.len() <= MAX_NEW_STREAM_HANDLES_PER_VALUE
                    && record.stream_mappings.len() == unique_transport_ids.len()
                    && record.stream_mappings.len() == unique_mappings.len()
                    && record.stream_mappings.iter().all(|mapping| {
                        mapping.role == SessionStreamRoleV1::Input
                            && supported_handle(&mapping.handle)
                    })
                    && record
                        .stream_mappings
                        .iter()
                        .map(|mapping| &mapping.handle)
                        .eq(record.attempt.invocation.stream_handles.iter())
            }
            Self::Mapping(record) => supported_handle(&record.mapping.handle),
            Self::AttachmentPrepared(record) => {
                record.key.is_well_formed()
                    && record.lease_expires_at_millis > record.prepared_at_millis
            }
            Self::AttachmentActivated(record) => {
                record.key.is_well_formed()
                    && record.lease_expires_at_millis > record.activated_at_millis
            }
            Self::AttachmentRenewed(record) => {
                record.key.is_well_formed()
                    && record.lease_expires_at_millis > record.renewed_at_millis
            }
            Self::AttachmentFinalized(record) => record.key.is_well_formed(),
            Self::ProducerDeleting(record) => !record.producer_fingerprint.0.is_nil(),
            Self::CascadeOutbox(record) => record.key.is_well_formed(),
            Self::ConsumerDeleting(record) => !record.consumer_fingerprint.0.is_nil(),
            Self::SourceUnavailable(record) => {
                record.key.is_well_formed()
                    && StreamOffsetV1::from_bytes(record.source_offset.0).is_ok()
            }
            Self::TopologyPrepared(record) => {
                record.attachment.is_well_formed()
                    && record.session_key == record.attachment.session_key
                    && topology_mapping_matches(&record.attachment, &record.mapping)
            }
            Self::TopologyActivated(record) => {
                record.attachment.is_well_formed()
                    && record.session_key == record.attachment.session_key
                    && topology_mapping_matches(&record.attachment, &record.mapping)
            }
            Self::InputHighWater(record) => {
                StreamOffsetV1::from_bytes(record.high_water.resulting_offset.0).is_ok()
            }
            Self::ConsumerItemValue(record) => {
                let unique_transport_ids = record
                    .recursive_mappings
                    .iter()
                    .map(|mapping| mapping.transport_stream_id)
                    .collect::<HashSet<_>>();
                let unique_mappings = record
                    .recursive_mappings
                    .iter()
                    .map(|mapping| (mapping.handle.clone(), mapping.role))
                    .collect::<HashSet<_>>();
                StreamOffsetV1::from_bytes(record.source_offset.0).is_ok()
                    && record.value.len() <= MAX_DURABLE_STREAM_ITEM_SIZE
                    && record.recursive_handles.len() <= MAX_NEW_STREAM_HANDLES_PER_VALUE
                    && record.recursive_handles.iter().all(supported_handle)
                    && record.recursive_mappings.len() == record.recursive_handles.len()
                    && record.recursive_mappings.len() == unique_transport_ids.len()
                    && record.recursive_mappings.len() == unique_mappings.len()
                    && record
                        .recursive_mappings
                        .iter()
                        .all(|mapping| supported_handle(&mapping.handle))
                    && record
                        .recursive_mappings
                        .iter()
                        .map(|mapping| &mapping.handle)
                        .eq(record.recursive_handles.iter())
                    && if record.packed_u8 {
                        record.value.len() == 1 && record.recursive_handles.is_empty()
                    } else {
                        true
                    }
            }
            Self::ConsumerCancelIntent(record) => record.epoch > 0,
            Self::ConsumerTerminal(record) => {
                StreamOffsetV1::from_bytes(record.source_offset.0).is_ok()
            }
            Self::InvocationResult(record) => {
                let unique_transport_ids = record
                    .stream_mappings
                    .iter()
                    .map(|mapping| mapping.transport_stream_id)
                    .collect::<HashSet<_>>();
                let unique_mappings = record
                    .stream_mappings
                    .iter()
                    .map(|mapping| (mapping.handle.clone(), mapping.role))
                    .collect::<HashSet<_>>();
                record.stream_mappings.len() <= MAX_NEW_STREAM_HANDLES_PER_VALUE
                    && record.stream_mappings.len() == unique_transport_ids.len()
                    && record.stream_mappings.len() == unique_mappings.len()
                    && record.output_streams.iter().all(supported_handle)
                    && record.stream_mappings.iter().all(|mapping| {
                        mapping.role == SessionStreamRoleV1::Output
                            && supported_handle(&mapping.handle)
                    })
                    && record
                        .stream_mappings
                        .iter()
                        .map(|mapping| &mapping.handle)
                        .eq(record.output_streams.iter())
            }
            Self::CallerAttempt(record) => {
                record.attempt_id.0.get_version() == Some(uuid::Version::Random)
                    && !record.attempt_id.0.is_nil()
            }
            Self::Attached(record) => {
                record.epoch > 0
                    && record.attempt_id.0.get_version() == Some(uuid::Version::Random)
                    && !record.attempt_id.0.is_nil()
                    && record.pending_invocation_oplog_index.is_defined()
                    && AttachmentId::primary(
                        record.session_key.callee_environment_id,
                        &record.session_key.callee,
                        &record.session_key.idempotency_key,
                    )
                    .is_ok_and(|attachment_id| attachment_id == record.attachment_id)
            }
            Self::ResumeAttempt(record) => {
                let attempt = &record.attempt;
                let unique_cursors = attempt
                    .cursors
                    .iter()
                    .map(|cursor| cursor.stream_id)
                    .collect::<HashSet<_>>();
                record.accepted_epoch == attempt.expected_epoch.checked_add(1).unwrap_or_default()
                    && attempt.format_version == DURABLE_STREAM_FORMAT_VERSION
                    && attempt.expected_epoch > 0
                    && attempt.attempt_id.0.get_version() == Some(uuid::Version::Random)
                    && !attempt.attempt_id.0.is_nil()
                    && attempt.expected_callee_fingerprint == attempt.session_key.callee_fingerprint
                    && attempt.cursors.len() == unique_cursors.len()
                    && attempt
                        .cursors
                        .windows(2)
                        .all(|pair| pair[0].stream_id.0.as_bytes() < pair[1].stream_id.0.as_bytes())
                    && usize::try_from(attempt.live_join_buffer_events).is_ok_and(|capacity| {
                        (MIN_LIVE_JOIN_BUFFER_SIZE..=MAX_LIVE_JOIN_BUFFER_SIZE).contains(&capacity)
                    })
                    && AttachmentId::primary(
                        attempt.session_key.callee_environment_id,
                        &attempt.session_key.callee,
                        &attempt.session_key.idempotency_key,
                    )
                    .is_ok_and(|attachment_id| attachment_id == attempt.attachment_id)
            }
            Self::Detached(record) => {
                record.epoch > 0
                    && record.owner_attempt_id.0.get_version() == Some(uuid::Version::Random)
                    && !record.owner_attempt_id.0.is_nil()
                    && AttachmentId::primary(
                        record.session_key.callee_environment_id,
                        &record.session_key.callee,
                        &record.session_key.idempotency_key,
                    )
                    .is_ok_and(|attachment_id| attachment_id == record.attachment_id)
            }
            Self::Finished(_) => true,
        }
    }
}

fn topology_mapping_matches(
    attachment: &StreamAttachmentKeyV1,
    mapping: &StreamSessionMappingRecordV1,
) -> bool {
    mapping.handle.format_version == DURABLE_STREAM_FORMAT_VERSION
        && mapping.handle.stream_id == attachment.stream_id
        && mapping.handle.producer_environment_id == attachment.producer_environment_id
        && mapping.handle.producer == attachment.producer
        && mapping.handle.expected_producer_fingerprint == attachment.expected_producer_fingerprint
}

#[cfg(test)]
mod tests {
    use super::{
        AttachmentId, AttemptId, DurableStreamHandleV1, PersistedStreamInvocationDescriptorV1,
        StartAttemptDescriptorV1, StreamAttachmentKeyV1, StreamId, StreamInvocationIdV1,
        StreamOffsetError, StreamOffsetV1, StreamSessionPreparedRecordV1, StreamSessionRecordV1,
    };
    use crate::base_model::component::{ComponentId, ComponentRevision};
    use crate::base_model::environment::EnvironmentId;
    use crate::base_model::{AgentFingerprint, AgentId, IdempotencyKey, OplogIndex};
    use golem_schema::schema::SchemaFingerprintV1;
    use test_r::test;
    use uuid::Uuid;

    #[test]
    fn stream_and_attachment_id_golden_vectors() {
        let environment_id =
            EnvironmentId(Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap());
        let agent_id = AgentId {
            component_id: ComponentId(
                Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap(),
            ),
            agent_id: "cart(42)".to_string(),
        };
        let fingerprint =
            AgentFingerprint(Uuid::parse_str("00000000-0000-0000-0000-000000000003").unwrap());
        assert_eq!(
            StreamId::derive(
                environment_id,
                &agent_id,
                fingerprint,
                OplogIndex::from_u64(42),
            )
            .unwrap()
            .to_string(),
            "d750525f-cc5d-5409-9576-46b1da257fbc"
        );
        assert_eq!(
            AttachmentId::primary(
                environment_id,
                &agent_id,
                &IdempotencyKey::new("550e8400-e29b-41d4-a716-446655440000".to_string()),
            )
            .unwrap()
            .to_string(),
            "59dee39e-df8f-59e5-ba06-4e5e04b5181f"
        );
        let attempt_id = AttemptId::fresh();
        assert_eq!(attempt_id.0.get_version(), Some(uuid::Version::Random));
        assert!(!attempt_id.0.is_nil());
    }

    #[test]
    fn stream_offset_layout_and_validation() {
        let offset = StreamOffsetV1::new(OplogIndex::from_u64(0x0102_0304_0506_0708), 0x090a0b0c);
        assert_eq!(offset.as_bytes()[0], 1);
        assert_eq!(&offset.as_bytes()[1..8], &[0; 7]);
        assert_eq!(
            offset.producer_oplog_index().as_u64(),
            0x0102_0304_0506_0708
        );
        assert_eq!(offset.sub_index(), 0x090a0b0c);
        assert_eq!(&offset.as_bytes()[20..24], &[0; 4]);

        let mut invalid = *offset.as_bytes();
        invalid[23] = 1;
        assert_eq!(
            StreamOffsetV1::from_bytes(invalid),
            Err(StreamOffsetError::ReservedBitsSet)
        );
    }

    #[test]
    fn attachment_identity_uses_session_authority_independently_of_consumer_identity() {
        let producer_environment_id = EnvironmentId(Uuid::from_u128(1));
        let producer = AgentId {
            component_id: ComponentId(Uuid::from_u128(2)),
            agent_id: "producer-c".to_string(),
        };
        let producer_fingerprint = AgentFingerprint(Uuid::from_u128(3));
        let session_environment_id = EnvironmentId(Uuid::from_u128(11));
        let session_authority = AgentId {
            component_id: ComponentId(Uuid::from_u128(12)),
            agent_id: "session-b".to_string(),
        };
        let session_key = StreamInvocationIdV1 {
            callee_environment_id: session_environment_id,
            callee: session_authority.clone(),
            callee_fingerprint: AgentFingerprint(Uuid::from_u128(13)),
            idempotency_key: IdempotencyKey::new("child-session".to_string()),
        };
        let consumer_environment_id = EnvironmentId(Uuid::from_u128(21));
        let consumer = AgentId {
            component_id: ComponentId(Uuid::from_u128(22)),
            agent_id: "consumer-a".to_string(),
        };
        let consumer_fingerprint = AgentFingerprint(Uuid::from_u128(23));
        let consumer_invocation = StreamInvocationIdV1 {
            callee_environment_id: consumer_environment_id,
            callee: consumer.clone(),
            callee_fingerprint: consumer_fingerprint,
            idempotency_key: IdempotencyKey::new("parent-invocation".to_string()),
        };
        let attachment_id = AttachmentId::primary(
            session_environment_id,
            &session_authority,
            &session_key.idempotency_key,
        )
        .unwrap();
        let key = StreamAttachmentKeyV1 {
            attachment_id,
            stream_id: StreamId(Uuid::from_u128(4)),
            epoch: 1,
            session_key,
            producer_environment_id,
            producer,
            expected_producer_fingerprint: producer_fingerprint,
            consumer_environment_id,
            consumer: consumer.clone(),
            expected_consumer_fingerprint: consumer_fingerprint,
            consumer_invocation,
        };

        assert!(key.is_well_formed());
        assert_ne!(
            attachment_id,
            AttachmentId::primary(
                consumer_environment_id,
                &consumer,
                &IdempotencyKey::new("parent-invocation".to_string()),
            )
            .unwrap()
        );

        let mut relabelled_consumer = key.clone();
        relabelled_consumer.consumer_invocation.callee_fingerprint =
            AgentFingerprint(Uuid::from_u128(24));
        assert!(!relabelled_consumer.is_well_formed());

        let mut relabelled_session = key;
        relabelled_session.session_key.idempotency_key =
            IdempotencyKey::new("different-child-session".to_string());
        assert!(!relabelled_session.is_well_formed());
    }

    // PROVISIONAL bug_finder reproducer — remove if the finding is rejected.
    #[test]
    fn prepared_record_rejects_unsupported_invocation_handle_version() {
        let environment_id = EnvironmentId(Uuid::from_u128(1));
        let agent_id = AgentId {
            component_id: ComponentId(Uuid::from_u128(2)),
            agent_id: "agent".to_string(),
        };
        let fingerprint = AgentFingerprint(Uuid::from_u128(3));
        let idempotency_key = IdempotencyKey::new("invocation".to_string());
        let session_key = StreamInvocationIdV1 {
            callee_environment_id: environment_id,
            callee: agent_id.clone(),
            callee_fingerprint: fingerprint,
            idempotency_key,
        };
        let unsupported_handle = DurableStreamHandleV1 {
            format_version: 2,
            stream_id: StreamId(Uuid::from_u128(4)),
            producer_environment_id: environment_id,
            producer: agent_id.clone(),
            expected_producer_fingerprint: fingerprint,
            source_invocation: session_key.clone(),
            component_revision: ComponentRevision::new(1).unwrap(),
            element_schema_fingerprint: SchemaFingerprintV1([0; 32]),
        };
        let record = StreamSessionRecordV1::Prepared(StreamSessionPreparedRecordV1 {
            format_version: 1,
            attempt: StartAttemptDescriptorV1 {
                format_version: 1,
                session_key: session_key.clone(),
                attachment_id: AttachmentId(Uuid::from_u128(5)),
                expected_callee_fingerprint: fingerprint,
                attempt_id: AttemptId(Uuid::from_u128(6)),
                invocation: PersistedStreamInvocationDescriptorV1 {
                    format_version: 1,
                    session_key,
                    target_component_revision: ComponentRevision::new(1).unwrap(),
                    method_name: "method".to_string(),
                    invocation_value: Vec::new(),
                    stream_handles: vec![unsupported_handle],
                    execution_config: Vec::new(),
                    effective_identity: Vec::new(),
                },
                effective_identity: Vec::new(),
                live_join_buffer_events: 32,
            },
            stream_mappings: Vec::new(),
        });

        assert!(
            !record.has_supported_format(),
            "a prepared v1 record must reject a non-v1 handle in its persisted invocation descriptor"
        );
    }
}
