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
use crate::base_model::regions::OplogRegion;
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
pub struct StreamOffset(pub [u8; 24]);

impl IntoSchemaTrait for StreamOffset {
    fn type_id() -> TypeId {
        TypeId::new("golem_common.base_model.StreamOffset")
    }

    fn register_in(builder: &mut SchemaBuilder) -> SchemaType {
        <Vec<u8> as IntoSchemaTrait>::register_in(builder)
    }

    fn to_value(&self) -> SchemaValue {
        self.0.to_vec().to_value()
    }
}

impl FromSchemaTrait for StreamOffset {
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

impl StreamOffset {
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

impl Display for StreamOffset {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl std::str::FromStr for StreamOffset {
    type Err = StreamOffsetError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 48 {
            return Err(StreamOffsetError::InvalidTextLength(value.len()));
        }

        let mut bytes = [0u8; 24];
        for (index, pair) in value.as_bytes().as_chunks::<2>().0.iter().enumerate() {
            let high = decode_lowercase_hex_digit(pair[0])?;
            let low = decode_lowercase_hex_digit(pair[1])?;
            bytes[index] = (high << 4) | low;
        }
        Self::from_bytes(bytes)
    }
}

fn decode_lowercase_hex_digit(byte: u8) -> Result<u8, StreamOffsetError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Err(StreamOffsetError::NonCanonicalHex),
        _ => Err(StreamOffsetError::InvalidHexCharacter(byte)),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum StreamOffsetError {
    #[error("stream offset text must have 48 characters, got {0}")]
    InvalidTextLength(usize),
    #[error("stream offset text must use lowercase hexadecimal characters")]
    NonCanonicalHex,
    #[error("invalid stream offset hexadecimal character {0:?}")]
    InvalidHexCharacter(u8),
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
pub struct StreamInvocationId {
    pub callee_environment_id: EnvironmentId,
    pub callee: AgentId,
    pub callee_fingerprint: AgentFingerprint,
    pub idempotency_key: IdempotencyKey,
}

pub type StreamSessionKey = StreamInvocationId;

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
pub struct DurableStreamHandle {
    pub format_version: u8,
    pub stream_id: StreamId,
    pub producer_environment_id: EnvironmentId,
    pub producer: AgentId,
    pub expected_producer_fingerprint: AgentFingerprint,
    pub producer_generation: OplogIndex,
    pub source_invocation: StreamInvocationId,
    pub component_revision: ComponentRevision,
    pub element_schema_fingerprint: SchemaFingerprintV1,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
pub enum StreamRegistrationCoordinate {
    Root {
        invocation_id: StreamInvocationId,
        root_kind: StreamRootKind,
        recursive_value_path: Vec<StreamValuePathStep>,
    },
    Nested {
        parent_stream_id: StreamId,
        parent_producer_sequence: u64,
        recursive_value_path: Vec<StreamValuePathStep>,
    },
}

#[derive(
    Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamRootKind {
    MethodInput,
    MethodResult,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum StreamValuePathStep {
    RecordField(u32),
    VariantCasePayload(u32),
    TupleElement(u32),
    ListElement(u32),
    FixedListElement(u32),
    MapEntry { index: u32, side: StreamMapSide },
    OptionSome,
    ResultOk,
    ResultErr,
    UnionBranch(u32),
}

#[derive(
    Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamMapSide {
    Key,
    Value,
}

#[derive(
    Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamSourceKind {
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
pub enum SessionStreamRole {
    Input,
    Output,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
#[serde(rename_all = "camelCase")]
pub struct StreamSessionMapping {
    pub session_key: StreamSessionKey,
    pub attachment_id: AttachmentId,
    pub role: SessionStreamRole,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamRegistrationInvocation {
    Local(IdempotencyKey),
    Remote(StreamInvocationId),
}

impl StreamRegistrationInvocation {
    pub fn idempotency_key(&self) -> &IdempotencyKey {
        match self {
            Self::Local(idempotency_key) => idempotency_key,
            Self::Remote(invocation) => &invocation.idempotency_key,
        }
    }

    pub fn qualify(
        &self,
        owner_environment_id: EnvironmentId,
        owner: &AgentId,
        owner_fingerprint: AgentFingerprint,
    ) -> StreamSessionKey {
        match self {
            Self::Local(idempotency_key) => StreamSessionKey {
                callee_environment_id: owner_environment_id,
                callee: owner.clone(),
                callee_fingerprint: owner_fingerprint,
                idempotency_key: idempotency_key.clone(),
            },
            Self::Remote(invocation) => invocation.clone(),
        }
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
pub struct LocalStreamId(pub OplogIndex);

/// One reader introduced by a binding table in this agent's oplog.
#[derive(
    Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct LocalStreamReaderId {
    pub introducing_oplog_index: OplogIndex,
    pub binding_slot: u32,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamRecordReference {
    Local(LocalStreamId),
    Foreign(DurableStreamHandle),
}

impl StreamRecordReference {
    pub fn has_supported_format(&self) -> bool {
        match self {
            Self::Local(id) => id.0.is_defined(),
            Self::Foreign(handle) => handle.format_version == DURABLE_STREAM_FORMAT_VERSION,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamRegistrationRecordCoordinate {
    Root {
        invocation: StreamRegistrationInvocation,
        root_kind: StreamRootKind,
        recursive_value_path: Vec<StreamValuePathStep>,
    },
    Nested {
        parent_stream: StreamRecordReference,
        parent_producer_sequence: u64,
        recursive_value_path: Vec<StreamValuePathStep>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamRegisteredRecord {
    pub format_version: u8,
    pub coordinate: StreamRegistrationRecordCoordinate,
    pub source_invocation: StreamRegistrationInvocation,
    pub component_revision: ComponentRevision,
    pub element_schema_fingerprint: SchemaFingerprintV1,
    pub source_kind: StreamSourceKind,
    pub session_role: Option<SessionStreamRole>,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamItemsRecord {
    pub format_version: u8,
    pub stream_id: LocalStreamId,
    pub first_sequence: u64,
    pub nested_stream_ids: Vec<StreamRecordReference>,
    pub newly_registered_stream_ids: Vec<LocalStreamId>,
    pub payload: StreamItemsPayload,
    pub offsets: Vec<StreamOffset>,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamItemsPayload {
    /// Canonically encoded complete durable values. V1 normally stores one value per entry.
    Values(Vec<Vec<u8>>),
    PackedU8(Vec<u8>),
}

impl StreamItemsPayload {
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
pub struct StreamEndRecord {
    pub format_version: u8,
    pub stream_id: LocalStreamId,
    pub sequence: u64,
    pub offset: StreamOffset,
    pub authored_by: StreamTerminalAuthor,
    pub result: StreamEndResult,
}

#[derive(
    Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamTerminalAuthor {
    Guest,
    Protocol,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamEndResult {
    Ok,
    ErrorContext(Vec<u8>),
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamCancelRecord {
    pub format_version: u8,
    pub stream_id: LocalStreamId,
    pub sequence: u64,
    pub offset: StreamOffset,
    pub authored_by: StreamTerminalAuthor,
    pub role: StreamCancelRole,
    pub reason: StreamCancelReason,
    pub details: Option<String>,
}

#[derive(
    Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamCancelRole {
    InputProducer,
    InputConsumer,
    OutputProducer,
    OutputConsumer,
    System,
}

#[derive(
    Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema,
)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamCancelReason {
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
pub struct PersistedStreamInvocationDescriptor {
    pub format_version: u8,
    pub session_key: StreamSessionKey,
    pub target_component_revision: ComponentRevision,
    pub target: PersistedInvocationTarget,
    /// Canonical serialization of the complete recursive invocation value after replacing each
    /// stream leaf with its corresponding durable handle in `stream_handles`. The typed-value
    /// bytes contain both the schema graph and value.
    pub invocation_value: Vec<u8>,
    pub stream_handles: Vec<DurableStreamHandle>,
    /// Canonical execution-mode and configuration bytes that affect the call.
    pub execution_config: Vec<u8>,
    /// Canonical effective principal and grant identity. Credential bytes and expiry are excluded.
    pub effective_identity: Vec<u8>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum PersistedInvocationTarget {
    AgentMethod {
        method_name: String,
    },
    ExternalTool {
        tool_name: String,
        command_path: Vec<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StartAttemptDescriptor {
    pub format_version: u8,
    pub session_key: StreamSessionKey,
    pub attachment_id: AttachmentId,
    pub expected_callee_fingerprint: AgentFingerprint,
    pub attempt_id: AttemptId,
    pub invocation: PersistedStreamInvocationDescriptor,
    pub effective_identity: Vec<u8>,
    pub live_join_buffer_events: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamSessionPreparedRecord {
    pub format_version: u8,
    pub session_key: IdempotencyKey,
    pub attempt: StartAttemptDescriptor,
    pub stream_mappings: Vec<StreamBindingRecord>,
}

/// Owner-relative source bound to a transport slot in an oplog record.
#[derive(Clone, Debug, Eq, Hash, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamBindingRecord {
    pub transport_stream_id: u64,
    pub source: StreamRecordReference,
    pub role: SessionStreamRole,
}

impl StreamBindingRecord {
    /// A received mapping remains foreign even when its handle names the oplog owner.
    pub fn foreign(mapping: &StreamSessionMappingRecord) -> Self {
        Self {
            transport_stream_id: mapping.transport_stream_id,
            source: StreamRecordReference::Foreign(mapping.handle.clone()),
            role: mapping.role,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamSessionMappingRecord {
    /// Transport-local source reference used only to route frames within this attachment.
    pub transport_stream_id: u64,
    pub handle: DurableStreamHandle,
    pub role: SessionStreamRole,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamSessionMappingUpdateRecord {
    pub format_version: u8,
    pub session_key: StreamRegistrationInvocation,
    pub mapping: StreamBindingRecord,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamAttachmentKey {
    pub attachment_id: AttachmentId,
    pub stream_id: StreamId,
    pub epoch: u64,
    pub session_key: StreamSessionKey,
    pub producer_environment_id: EnvironmentId,
    pub producer: AgentId,
    pub expected_producer_fingerprint: AgentFingerprint,
    pub consumer_environment_id: EnvironmentId,
    pub consumer: AgentId,
    pub expected_consumer_fingerprint: AgentFingerprint,
    pub consumer_invocation: StreamInvocationId,
}

impl StreamAttachmentKey {
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
pub struct StreamAttachmentPreparedRecord {
    pub format_version: u8,
    pub key: StreamAttachmentKey,
    pub prepared_at_millis: u64,
    pub lease_expires_at_millis: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamAttachmentActivatedRecord {
    pub format_version: u8,
    pub key: StreamAttachmentKey,
    pub activated_at_millis: u64,
    pub lease_expires_at_millis: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamAttachmentRenewedRecord {
    pub format_version: u8,
    pub key: StreamAttachmentKey,
    pub renewed_at_millis: u64,
    pub lease_expires_at_millis: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamAttachmentFinalizationReason {
    ConsumerFinalized,
    ConsumerDeleted,
    ConsumerIncarnationChanged,
    PrepareAbandoned,
    Reconciled,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamAttachmentControlRequest {
    pub format_version: u8,
    pub mapping: Option<StreamSessionMappingRecord>,
    pub operation: StreamAttachmentControlOperation,
}

impl StreamAttachmentControlRequest {
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
pub struct AttachedStreamSegmentRequest {
    pub format_version: u8,
    pub attachment: StreamAttachmentKey,
    pub mapping: StreamSessionMappingRecord,
    pub after: Option<StreamOffset>,
    pub through: Option<StreamOffset>,
    pub wait_for_events: bool,
}

impl AttachedStreamSegmentRequest {
    pub fn is_well_formed(&self) -> bool {
        self.format_version == DURABLE_STREAM_FORMAT_VERSION
            && self.attachment.is_well_formed()
            && topology_mapping_matches(&self.attachment, &self.mapping)
            && (!self.wait_for_events || self.through.is_none())
    }
}

/// Internal delegation after the exporting worker has authorized and resolved a slot.
#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct StreamHandleReadRequest {
    pub handle: DurableStreamHandle,
    pub after: Option<StreamOffset>,
    pub max_items: u32,
    pub max_bytes: u64,
    pub wait_millis: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum DurableStreamReadRequest {
    AttachedConsumer(Box<AttachedStreamSegmentRequest>),
    AuthorizedExport(Box<StreamHandleReadRequest>),
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamAttachmentControlOperation {
    Prepare {
        key: StreamAttachmentKey,
        now_millis: u64,
    },
    Activate {
        key: StreamAttachmentKey,
        now_millis: u64,
    },
    Detach {
        key: StreamAttachmentKey,
    },
    Renew {
        key: StreamAttachmentKey,
        now_millis: u64,
    },
    Cancel {
        key: StreamAttachmentKey,
        role: StreamCancelRole,
        reason: StreamCancelReason,
        details: Option<String>,
    },
    Finalize {
        key: StreamAttachmentKey,
        reason: StreamAttachmentFinalizationReason,
        now_millis: u64,
    },
    SourceUnavailable {
        key: StreamAttachmentKey,
        reader_id: LocalStreamReaderId,
        source_offset: StreamOffset,
        consumer_read_ordinal: u64,
    },
}

impl StreamAttachmentControlOperation {
    pub fn key(&self) -> &StreamAttachmentKey {
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
pub struct StreamAttachmentFinalizedRecord {
    pub format_version: u8,
    pub key: StreamAttachmentKey,
    pub finalized_at_millis: u64,
    pub reason: StreamAttachmentFinalizationReason,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamProducerDeletingRecord {
    pub format_version: u8,
    pub producer_environment_id: EnvironmentId,
    pub producer: AgentId,
    pub producer_fingerprint: AgentFingerprint,
    pub deleting_at_millis: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamCascadeDependentResult {
    ConsumerJournalComplete,
    SourceUnavailable {
        first_unjournaled_offset: StreamOffset,
    },
    ConsumerDeleted,
    ConsumerIncarnationChanged,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamCascadeOutboxRecord {
    pub format_version: u8,
    pub key: StreamAttachmentKey,
    pub completed_at_millis: u64,
    pub result: StreamCascadeDependentResult,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamConsumerDeletingRecord {
    pub format_version: u8,
    pub consumer_environment_id: EnvironmentId,
    pub consumer: AgentId,
    pub consumer_fingerprint: AgentFingerprint,
    pub deleting_at_millis: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamSourceUnavailableRecord {
    pub format_version: u8,
    pub session_key: StreamRegistrationInvocation,
    pub reader_id: LocalStreamReaderId,
    pub source_offset: StreamOffset,
    pub consumer_read_ordinal: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamTopologyPreparedRecord {
    pub format_version: u8,
    pub session_key: StreamSessionKey,
    pub attachment: StreamAttachmentKey,
    pub mapping: StreamSessionMappingRecord,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamTopologyActivatedRecord {
    pub format_version: u8,
    pub session_key: StreamSessionKey,
    pub attachment: StreamAttachmentKey,
    pub mapping: StreamSessionMappingRecord,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamSessionAttachedRecord {
    pub format_version: u8,
    pub session_key: IdempotencyKey,
    pub attachment_id: AttachmentId,
    pub attempt_id: AttemptId,
    pub epoch: u64,
    pub pending_invocation_oplog_index: OplogIndex,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamResumeOperation {
    Resume,
    Takeover,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamResumeCursor {
    pub stream_id: StreamId,
    pub last_observed_offset: Option<StreamOffset>,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct ResumeAttemptDescriptor {
    pub format_version: u8,
    pub operation: StreamResumeOperation,
    pub session_key: StreamSessionKey,
    pub attachment_id: AttachmentId,
    pub expected_callee_fingerprint: AgentFingerprint,
    pub attempt_id: AttemptId,
    pub expected_epoch: u64,
    pub effective_identity: Vec<u8>,
    pub cursors: Vec<StreamResumeCursor>,
    pub live_join_buffer_events: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamSessionResumeAttemptRecord {
    pub format_version: u8,
    pub session_key: IdempotencyKey,
    pub attempt: ResumeAttemptDescriptor,
    pub accepted_epoch: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamSessionDetachedRecord {
    pub format_version: u8,
    pub session_key: IdempotencyKey,
    pub attachment_id: AttachmentId,
    pub owner_attempt_id: AttemptId,
    pub epoch: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct InputStreamHighWater {
    pub highest_contiguous_sequence: u64,
    pub resulting_offset: StreamOffset,
    pub terminal: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamSessionInputHighWaterRecord {
    pub format_version: u8,
    pub session_key: StreamSessionKey,
    pub stream_id: StreamId,
    pub epoch: u64,
    pub first_sequence: u64,
    pub payload: StreamItemsPayload,
    pub high_water: InputStreamHighWater,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamExternalProducerStateRecord {
    pub format_version: u8,
    pub session_key: StreamSessionKey,
    pub stream_id: StreamId,
    pub producer_id: ExternalProducerId,
    pub epoch: u64,
    pub sequence: u64,
    pub next_sequence: u64,
    pub resulting_offset: StreamOffset,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum ExternalProducerId {
    Client(String),
    Attached,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamConsumerItemValueRecord {
    pub format_version: u8,
    pub session_key: StreamRegistrationInvocation,
    pub reader_id: LocalStreamReaderId,
    pub source_offset: StreamOffset,
    pub consumer_read_ordinal: u64,
    pub value: Vec<u8>,
    pub packed_u8: bool,
    pub recursive_mappings: Vec<StreamBindingRecord>,
}

impl StreamConsumerItemValueRecord {
    pub fn logical_item_count(&self) -> usize {
        if self.packed_u8 { self.value.len() } else { 1 }
    }

    pub fn source_offset_at(&self, index: usize) -> Option<StreamOffset> {
        if index >= self.logical_item_count() {
            return None;
        }
        if !self.packed_u8 {
            return Some(self.source_offset);
        }
        let index = u32::try_from(index).ok()?;
        let sub_index = self.source_offset.sub_index().checked_add(index)?;
        Some(StreamOffset::new(
            self.source_offset.producer_oplog_index(),
            sub_index,
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamConsumerTerminalRecord {
    pub format_version: u8,
    pub session_key: StreamRegistrationInvocation,
    pub reader_id: LocalStreamReaderId,
    pub source_offset: StreamOffset,
    pub consumer_read_ordinal: u64,
    pub terminal: StreamConsumerTerminal,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamConsumerCancelIntentRecord {
    pub format_version: u8,
    pub session_key: StreamRegistrationInvocation,
    /// The consumer's invocation, relative to the oplog owner.
    pub consumer_invocation: IdempotencyKey,
    pub source: StreamRecordReference,
    pub epoch: u64,
    pub role: StreamCancelRole,
    pub reason: StreamCancelReason,
    pub details: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamConsumerCancelAppliedRecord {
    pub format_version: u8,
    pub intent: StreamConsumerCancelIntentRecord,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamConsumerTerminal {
    End(StreamEndResult),
    Cancel {
        role: StreamCancelRole,
        reason: StreamCancelReason,
        details: Option<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamSessionInvocationResultRecord {
    pub format_version: u8,
    pub session_key: StreamRegistrationInvocation,
    pub result: Vec<u8>,
    pub stream_mappings: Vec<StreamBindingRecord>,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamSessionFinishedRecord {
    pub format_version: u8,
    pub session_key: StreamRegistrationInvocation,
    pub result: Result<(), Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamSlotTombstonedRecord {
    pub format_version: u8,
    pub session_key: StreamRegistrationInvocation,
    pub slot: String,
    pub role: SessionStreamRole,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamSessionCancelRequestedRecord {
    pub format_version: u8,
    pub session_key: StreamRegistrationInvocation,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamCallerAttemptRecord {
    pub format_version: u8,
    pub session_key: StreamRegistrationInvocation,
    pub attempt_id: AttemptId,
}

/// A cut of the owner-local journal, with optional sub-item clipping and an HTTP creation receipt.
#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct StreamForkCutRecord {
    pub format_version: u8,
    /// Hash of the creation request, including the requested source generation and cut.
    pub request_hash: Vec<u8>,
    /// Binds the creation receipt to its target incarnation, not to a copied ancestor's receipt.
    pub creation_fingerprint: AgentFingerprint,
    pub export: Option<StreamExportFork>,
    pub cut_index: OplogIndex,
    /// Present only for a self-revert marker. The marker follows the physical `Revert` entry
    /// immediately after this deleted region.
    pub revert: Option<OplogRegion>,
    /// Minimum epoch for attachments issued by the continuation. A self-revert advances
    /// beyond epochs issued in the discarded history; a new agent starts at one.
    pub epoch_floor: u64,
    pub selected_stream_id: Option<LocalStreamId>,
    pub retained_through: Option<StreamOffset>,
}

/// Immutable public creation configuration, distinct from the resolved physical cut.
#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub struct StreamExportFork {
    pub source: AgentId,
    pub source_environment_id: EnvironmentId,
    pub source_fingerprint: AgentFingerprint,
    pub source_path: String,
    pub session: String,
    pub slot: String,
    pub expected_method: String,
    pub requested_offset: Option<StreamOffset>,
    pub anchor: Option<StreamOffset>,
    pub sub_offset: u64,
    pub content_type: String,
    pub initial_content_hash: Vec<u8>,
    pub closed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
pub enum StreamSessionRecord {
    CallerAttempt(StreamCallerAttemptRecord),
    Prepared(StreamSessionPreparedRecord),
    Attached(StreamSessionAttachedRecord),
    ResumeAttempt(StreamSessionResumeAttemptRecord),
    Detached(StreamSessionDetachedRecord),
    Mapping(StreamSessionMappingUpdateRecord),
    AttachmentPrepared(StreamAttachmentPreparedRecord),
    AttachmentActivated(StreamAttachmentActivatedRecord),
    AttachmentRenewed(StreamAttachmentRenewedRecord),
    AttachmentFinalized(StreamAttachmentFinalizedRecord),
    ProducerDeleting(StreamProducerDeletingRecord),
    CascadeOutbox(StreamCascadeOutboxRecord),
    ConsumerDeleting(StreamConsumerDeletingRecord),
    SourceUnavailable(StreamSourceUnavailableRecord),
    TopologyPrepared(StreamTopologyPreparedRecord),
    TopologyActivated(StreamTopologyActivatedRecord),
    InputHighWater(StreamSessionInputHighWaterRecord),
    ExternalProducerState(StreamExternalProducerStateRecord),
    ConsumerItemValue(StreamConsumerItemValueRecord),
    ConsumerCancelIntent(StreamConsumerCancelIntentRecord),
    ConsumerCancelApplied(StreamConsumerCancelAppliedRecord),
    ConsumerTerminal(StreamConsumerTerminalRecord),
    InvocationResult(StreamSessionInvocationResultRecord),
    Finished(StreamSessionFinishedRecord),
    Tombstoned(StreamSlotTombstonedRecord),
    CancelRequested(StreamSessionCancelRequestedRecord),
    ForkCut(StreamForkCutRecord),
}

impl StreamSessionRecord {
    /// Identifies records that can change this owner's callee-side session lifecycle.
    pub fn local_session_key(&self) -> Option<&IdempotencyKey> {
        let reference = match self {
            Self::Prepared(record) => return Some(&record.session_key),
            Self::Attached(record) => return Some(&record.session_key),
            Self::ResumeAttempt(record) => return Some(&record.session_key),
            Self::Detached(record) => return Some(&record.session_key),
            Self::InvocationResult(record) => &record.session_key,
            Self::Finished(record) => &record.session_key,
            Self::Tombstoned(record) => &record.session_key,
            Self::CancelRequested(record) => &record.session_key,
            Self::ConsumerCancelApplied(record) => &record.intent.session_key,
            _ => return None,
        };
        match reference {
            StreamRegistrationInvocation::Local(key) => Some(key),
            StreamRegistrationInvocation::Remote(_) => None,
        }
    }

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
            Self::ExternalProducerState(record) => record.format_version,
            Self::ConsumerItemValue(record) => record.format_version,
            Self::ConsumerCancelIntent(record) => record.format_version,
            Self::ConsumerCancelApplied(record) => record.format_version,
            Self::ConsumerTerminal(record) => record.format_version,
            Self::InvocationResult(record) => record.format_version,
            Self::Finished(record) => record.format_version,
            Self::Tombstoned(record) => record.format_version,
            Self::CancelRequested(record) => record.format_version,
            Self::ForkCut(record) => record.format_version,
        }
    }

    pub fn has_supported_format(&self) -> bool {
        fn supported_handle(handle: &DurableStreamHandle) -> bool {
            handle.format_version == DURABLE_STREAM_FORMAT_VERSION
        }

        fn supported_attempt(attempt: &StartAttemptDescriptor) -> bool {
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
                    .map(|mapping| (&mapping.source, mapping.role))
                    .collect::<HashSet<_>>();
                supported_attempt(&record.attempt)
                    && record.session_key == record.attempt.session_key.idempotency_key
                    && record
                        .attempt
                        .invocation
                        .stream_handles
                        .iter()
                        .all(supported_handle)
                    && record.stream_mappings.len() <= MAX_NEW_STREAM_HANDLES_PER_VALUE
                    && record.stream_mappings.len() == unique_transport_ids.len()
                    && record.stream_mappings.len() == unique_mappings.len()
                    && record
                        .stream_mappings
                        .iter()
                        .all(|mapping| mapping.source.has_supported_format())
                    && record
                        .stream_mappings
                        .iter()
                        .filter(|mapping| mapping.role == SessionStreamRole::Input)
                        .count()
                        == record.attempt.invocation.stream_handles.len()
                    && record
                        .stream_mappings
                        .iter()
                        .filter(|mapping| mapping.role == SessionStreamRole::Input)
                        .zip(&record.attempt.invocation.stream_handles)
                        .all(|(binding, original)| match &binding.source {
                            StreamRecordReference::Local(_) => true,
                            StreamRecordReference::Foreign(handle) => handle == original,
                        })
            }
            Self::Mapping(record) => record.mapping.source.has_supported_format(),
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
                StreamOffset::from_bytes(record.source_offset.0).is_ok()
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
                StreamOffset::from_bytes(record.high_water.resulting_offset.0).is_ok()
            }
            Self::ExternalProducerState(record) => {
                !matches!(&record.producer_id, ExternalProducerId::Client(id) if id.is_empty())
                    && record.next_sequence > record.sequence
                    && StreamOffset::from_bytes(record.resulting_offset.0).is_ok()
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
                    .map(|mapping| (&mapping.source, mapping.role))
                    .collect::<HashSet<_>>();
                StreamOffset::from_bytes(record.source_offset.0).is_ok()
                    && record.value.len() <= MAX_DURABLE_STREAM_ITEM_SIZE
                    && record.recursive_mappings.len() <= MAX_NEW_STREAM_HANDLES_PER_VALUE
                    && record.recursive_mappings.len() == unique_transport_ids.len()
                    && record.recursive_mappings.len() == unique_mappings.len()
                    && record
                        .recursive_mappings
                        .iter()
                        .all(|mapping| mapping.source.has_supported_format())
                    && if record.packed_u8 {
                        !record.value.is_empty()
                            && record
                                .source_offset_at(record.value.len().saturating_sub(1))
                                .is_some()
                            && record.recursive_mappings.is_empty()
                    } else {
                        true
                    }
            }
            Self::ConsumerCancelIntent(record) => {
                record.epoch > 0 && record.source.has_supported_format()
            }
            Self::ConsumerCancelApplied(record) => {
                record.intent.format_version == DURABLE_STREAM_FORMAT_VERSION
                    && record.intent.epoch > 0
                    && record.intent.source.has_supported_format()
            }
            Self::ConsumerTerminal(record) => {
                StreamOffset::from_bytes(record.source_offset.0).is_ok()
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
                    .map(|mapping| (&mapping.source, mapping.role))
                    .collect::<HashSet<_>>();
                record.stream_mappings.len() <= MAX_NEW_STREAM_HANDLES_PER_VALUE
                    && record.stream_mappings.len() == unique_transport_ids.len()
                    && record.stream_mappings.len() == unique_mappings.len()
                    && record.stream_mappings.iter().all(|mapping| {
                        mapping.role == SessionStreamRole::Output
                            && mapping.source.has_supported_format()
                    })
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
            }
            Self::ResumeAttempt(record) => {
                let attempt = &record.attempt;
                let unique_cursors = attempt
                    .cursors
                    .iter()
                    .map(|cursor| cursor.stream_id)
                    .collect::<HashSet<_>>();
                record.accepted_epoch == attempt.expected_epoch.checked_add(1).unwrap_or_default()
                    && record.session_key == attempt.session_key.idempotency_key
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
            }
            Self::Finished(_) => true,
            Self::Tombstoned(record) => !record.slot.is_empty(),
            Self::CancelRequested(_) => true,
            Self::ForkCut(record) => {
                let valid_revert = record.revert.as_ref().is_none_or(|region| {
                    record.cut_index.as_u64().checked_add(1) == Some(region.start.as_u64())
                        && region.start <= region.end
                        && record.selected_stream_id.is_none()
                        && record.retained_through.is_none()
                });
                record.request_hash.len() == 32
                    && record.epoch_floor > 0
                    && record.cut_index > OplogIndex::NONE
                    && valid_revert
                    && record
                        .selected_stream_id
                        .is_none_or(|id| id.0.is_defined() && id.0 <= record.cut_index)
                    && record.retained_through.is_none_or(|offset| {
                        record.selected_stream_id.is_some()
                            && StreamOffset::from_bytes(offset.0).is_ok()
                            && offset.producer_oplog_index() > OplogIndex::NONE
                            && offset.producer_oplog_index() <= record.cut_index
                    })
            }
        }
    }
}

fn topology_mapping_matches(
    attachment: &StreamAttachmentKey,
    mapping: &StreamSessionMappingRecord,
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
        AttachmentId, AttemptId, DurableStreamHandle, LocalStreamReaderId,
        PersistedInvocationTarget, PersistedStreamInvocationDescriptor, SessionStreamRole,
        StartAttemptDescriptor, StreamAttachmentKey, StreamBindingRecord,
        StreamConsumerItemValueRecord, StreamId, StreamInvocationId, StreamOffset,
        StreamOffsetError, StreamRecordReference, StreamSessionPreparedRecord, StreamSessionRecord,
    };
    use crate::base_model::component::{ComponentId, ComponentRevision};
    use crate::base_model::environment::EnvironmentId;
    use crate::base_model::{AgentFingerprint, AgentId, IdempotencyKey, OplogIndex};
    use golem_schema::schema::SchemaFingerprintV1;
    use proptest::prelude::*;
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
        let offset = StreamOffset::new(OplogIndex::from_u64(0x0102_0304_0506_0708), 0x090a0b0c);
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
            StreamOffset::from_bytes(invalid),
            Err(StreamOffsetError::ReservedBitsSet)
        );
    }

    proptest! {
        #[test]
        fn stream_offset_display_from_str_roundtrip(oplog_index: u64, sub_index: u32) {
            let offset = StreamOffset::new(OplogIndex::from_u64(oplog_index), sub_index);
            let displayed = offset.to_string();

            prop_assert_eq!(displayed.len(), 48);
            prop_assert!(displayed.bytes().all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')));
            prop_assert_eq!(displayed.parse::<StreamOffset>()?, offset);
        }

        #[test]
        fn stream_offset_display_preserves_lexical_order(
            left_oplog_index: u64,
            left_sub_index: u32,
            right_oplog_index: u64,
            right_sub_index: u32,
        ) {
            let left = StreamOffset::new(OplogIndex::from_u64(left_oplog_index), left_sub_index);
            let right = StreamOffset::new(OplogIndex::from_u64(right_oplog_index), right_sub_index);

            prop_assert_eq!(left.cmp(&right), left.to_string().cmp(&right.to_string()));
        }
    }

    #[test]
    fn stream_offset_from_str_rejects_invalid_text() {
        let canonical = StreamOffset::new(OplogIndex::from_u64(42), 7).to_string();

        assert_eq!(
            canonical[..47].parse::<StreamOffset>(),
            Err(StreamOffsetError::InvalidTextLength(47))
        );
        assert_eq!(
            format!("{canonical}0").parse::<StreamOffset>(),
            Err(StreamOffsetError::InvalidTextLength(49))
        );

        let mut uppercase = canonical.clone().into_bytes();
        uppercase[31] = b'A';
        assert_eq!(
            String::from_utf8(uppercase)
                .unwrap()
                .parse::<StreamOffset>(),
            Err(StreamOffsetError::NonCanonicalHex)
        );

        let mut nonhex = canonical.clone().into_bytes();
        nonhex[31] = b'g';
        assert_eq!(
            String::from_utf8(nonhex).unwrap().parse::<StreamOffset>(),
            Err(StreamOffsetError::InvalidHexCharacter(b'g'))
        );

        let unsupported_version = format!("02{}", &canonical[2..]);
        assert_eq!(
            unsupported_version.parse::<StreamOffset>(),
            Err(StreamOffsetError::UnsupportedVersion(2))
        );

        let reserved_bits = format!("0101{}", &canonical[4..]);
        assert_eq!(
            reserved_bits.parse::<StreamOffset>(),
            Err(StreamOffsetError::ReservedBitsSet)
        );
    }

    #[test]
    fn packed_consumer_journal_record_derives_logical_offsets() {
        let environment_id = EnvironmentId(Uuid::from_u128(1));
        let session_key = StreamInvocationId {
            callee_environment_id: environment_id,
            callee: AgentId {
                component_id: ComponentId(Uuid::from_u128(2)),
                agent_id: "consumer".to_string(),
            },
            callee_fingerprint: AgentFingerprint(Uuid::from_u128(3)),
            idempotency_key: IdempotencyKey::new("invocation".to_string()),
        };
        let mut record = StreamConsumerItemValueRecord {
            format_version: 1,
            session_key: super::StreamRegistrationInvocation::Remote(session_key),
            reader_id: LocalStreamReaderId {
                introducing_oplog_index: OplogIndex::from_u64(4),
                binding_slot: 0,
            },
            source_offset: StreamOffset::new(OplogIndex::from_u64(5), 7),
            consumer_read_ordinal: 0,
            value: vec![10, 11, 12],
            packed_u8: true,
            recursive_mappings: Vec::new(),
        };

        assert_eq!(record.logical_item_count(), 3);
        assert_eq!(record.source_offset_at(0).unwrap().sub_index(), 7);
        assert_eq!(record.source_offset_at(2).unwrap().sub_index(), 9);
        assert!(record.source_offset_at(3).is_none());
        assert!(StreamSessionRecord::ConsumerItemValue(record.clone()).has_supported_format());

        record.value.clear();
        assert!(!StreamSessionRecord::ConsumerItemValue(record.clone()).has_supported_format());

        record.value = vec![10, 11, 12];
        record.source_offset = StreamOffset::new(OplogIndex::from_u64(5), u32::MAX - 1);
        assert!(!StreamSessionRecord::ConsumerItemValue(record).has_supported_format());
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
        let session_key = StreamInvocationId {
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
        let consumer_invocation = StreamInvocationId {
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
        let key = StreamAttachmentKey {
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
        let session_key = StreamInvocationId {
            callee_environment_id: environment_id,
            callee: agent_id.clone(),
            callee_fingerprint: fingerprint,
            idempotency_key,
        };
        let unsupported_handle = DurableStreamHandle {
            format_version: 2,
            stream_id: StreamId(Uuid::from_u128(4)),
            producer_environment_id: environment_id,
            producer: agent_id.clone(),
            expected_producer_fingerprint: fingerprint,
            producer_generation: OplogIndex::NONE,
            source_invocation: session_key.clone(),
            component_revision: ComponentRevision::new(1).unwrap(),
            element_schema_fingerprint: SchemaFingerprintV1([0; 32]),
        };
        let record = StreamSessionRecord::Prepared(StreamSessionPreparedRecord {
            format_version: 1,
            session_key: session_key.idempotency_key.clone(),
            attempt: StartAttemptDescriptor {
                format_version: 1,
                session_key: session_key.clone(),
                attachment_id: AttachmentId(Uuid::from_u128(5)),
                expected_callee_fingerprint: fingerprint,
                attempt_id: AttemptId(Uuid::from_u128(6)),
                invocation: PersistedStreamInvocationDescriptor {
                    format_version: 1,
                    session_key,
                    target_component_revision: ComponentRevision::new(1).unwrap(),
                    target: PersistedInvocationTarget::AgentMethod {
                        method_name: "method".to_string(),
                    },
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

    #[test]
    fn prepared_record_validates_input_handles_and_allows_early_outputs_for_any_target() {
        let environment_id = EnvironmentId(Uuid::from_u128(1));
        let agent_id = AgentId {
            component_id: ComponentId(Uuid::from_u128(2)),
            agent_id: "tool-host".to_string(),
        };
        let fingerprint = AgentFingerprint(Uuid::from_u128(3));
        let session_key = StreamInvocationId {
            callee_environment_id: environment_id,
            callee: agent_id.clone(),
            callee_fingerprint: fingerprint,
            idempotency_key: IdempotencyKey::new("tool-invocation".to_string()),
        };
        let attachment_id =
            AttachmentId::primary(environment_id, &agent_id, &session_key.idempotency_key).unwrap();
        let handle = |id| DurableStreamHandle {
            format_version: 1,
            stream_id: StreamId(Uuid::from_u128(id)),
            producer_generation: OplogIndex::NONE,
            producer_environment_id: environment_id,
            producer: agent_id.clone(),
            expected_producer_fingerprint: fingerprint,
            source_invocation: session_key.clone(),
            component_revision: ComponentRevision::INITIAL,
            element_schema_fingerprint: SchemaFingerprintV1([0; 32]),
        };
        let stdin = handle(4);
        let stdout = handle(5);
        let argument = handle(6);
        let mut prepared = StreamSessionPreparedRecord {
            format_version: 1,
            session_key: session_key.idempotency_key.clone(),
            attempt: StartAttemptDescriptor {
                format_version: 1,
                session_key: session_key.clone(),
                attachment_id,
                expected_callee_fingerprint: fingerprint,
                attempt_id: AttemptId::fresh(),
                invocation: PersistedStreamInvocationDescriptor {
                    format_version: 1,
                    session_key,
                    target_component_revision: ComponentRevision::INITIAL,
                    target: PersistedInvocationTarget::ExternalTool {
                        tool_name: "cat".to_string(),
                        command_path: vec!["/bin/cat".to_string()],
                    },
                    invocation_value: vec![],
                    stream_handles: vec![stdin.clone(), argument.clone()],
                    execution_config: vec![],
                    effective_identity: vec![],
                },
                effective_identity: vec![],
                live_join_buffer_events: 1,
            },
            stream_mappings: vec![
                StreamBindingRecord {
                    transport_stream_id: 10,
                    source: StreamRecordReference::Foreign(stdin),
                    role: SessionStreamRole::Input,
                },
                StreamBindingRecord {
                    transport_stream_id: 11,
                    source: StreamRecordReference::Foreign(stdout),
                    role: SessionStreamRole::Output,
                },
                StreamBindingRecord {
                    transport_stream_id: 12,
                    source: StreamRecordReference::Foreign(argument),
                    role: SessionStreamRole::Input,
                },
            ],
        };

        assert!(StreamSessionRecord::Prepared(prepared.clone()).has_supported_format());

        prepared.stream_mappings[0].role = SessionStreamRole::Output;
        assert!(!StreamSessionRecord::Prepared(prepared.clone()).has_supported_format());
        prepared.stream_mappings[0].role = SessionStreamRole::Input;
        prepared.stream_mappings[1].transport_stream_id = 10;
        assert!(!StreamSessionRecord::Prepared(prepared.clone()).has_supported_format());
        prepared.stream_mappings[1].transport_stream_id = 11;
        prepared.stream_mappings[2].role = SessionStreamRole::Output;
        assert!(!StreamSessionRecord::Prepared(prepared.clone()).has_supported_format());
        prepared.stream_mappings[2].role = SessionStreamRole::Input;
        prepared.attempt.invocation.target = PersistedInvocationTarget::AgentMethod {
            method_name: "run".to_string(),
        };
        assert!(StreamSessionRecord::Prepared(prepared).has_supported_format());
    }
}
