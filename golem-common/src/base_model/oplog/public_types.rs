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

use crate::base_model::card::{CardId, StoredCard};
use crate::base_model::component::{ComponentRevision, PluginPriority};
use crate::base_model::environment_plugin_grant::EnvironmentPluginGrantId;
use crate::base_model::invocation_context::{SpanId, TraceId};
use crate::base_model::oplog::PublicOplogEntry;
use crate::base_model::oplog::public_oplog_entry::{Deserialize, Serialize};
use crate::base_model::retry_policy::{ApiPredicate, ApiRetryPolicy};
use crate::base_model::{Empty, IdempotencyKey, OplogIndex, Timestamp};
use crate::declare_structs;
use crate::schema::TypedSchemaValue;
use golem_schema_derive::{FromSchema, IntoSchema};
use std::collections::BTreeMap;
use std::fmt;
use std::fmt::{Display, Formatter};

/// Public-oplog-local counterpart of `TypedAgentConfigEntry`. Both now carry a
/// schema-native `TypedSchemaValue`; this type exists as the public-oplog DTO
/// (poem/serde shape) and is produced at the public-oplog render edge.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PublicTypedAgentConfigEntry {
    pub path: Vec<String>,
    pub value: TypedSchemaValue,
}

#[cfg(feature = "full")]
impl TryFrom<PublicTypedAgentConfigEntry> for crate::base_model::worker::UntypedAgentConfigEntry {
    type Error = String;

    fn try_from(value: PublicTypedAgentConfigEntry) -> Result<Self, Self::Error> {
        let (_graph, schema_value) = value.value.into_parts();
        Ok(Self {
            path: value.path,
            value: schema_value,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct OplogCursor {
    pub next_oplog_index: u64,
    pub current_component_revision: u64,
}

impl Display for OplogCursor {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}-{}",
            self.next_oplog_index, self.current_component_revision
        )
    }
}

declare_structs! {
    pub struct PublicOplogEntryWithIndex {
        pub oplog_index: OplogIndex,
        pub entry: PublicOplogEntry,
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PluginInstallationDescription {
    pub environment_plugin_grant_id: EnvironmentPluginGrantId,
    pub plugin_priority: PluginPriority,
    pub plugin_name: String,
    pub plugin_version: String,
    pub parameters: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct WriteRemoteBatchedParameters {
    pub index: Option<OplogIndex>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct WriteRemoteTransactionParameters {
    pub index: Option<OplogIndex>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum PublicDurableFunctionType {
    /// The side-effect reads from the worker's local state (for example local file system,
    /// random generator, etc.)
    ReadLocal(Empty),
    /// The side-effect writes to the worker's local state (for example local file system)
    WriteLocal(Empty),
    /// The side-effect reads from external state (for example a key-value store)
    ReadRemote(Empty),
    /// The side-effect manipulates external state (for example an RPC call)
    WriteRemote(Empty),
    /// The side-effect manipulates external state through multiple invoked functions (for example
    /// a HTTP request where reading the response involves multiple host function calls)
    ///
    /// On the first invocation of the batch, the parameter should be `None` - this triggers
    /// writing a scope `Start` entry in the oplog. Followup invocations should contain
    /// this entry's index as the parameter. In batched remote writes it is the caller's responsibility
    /// to manually write the matching scope `End` entry (using `end_function`) when the operation is completed.
    WriteRemoteBatched(WriteRemoteBatchedParameters),
    WriteRemoteTransaction(WriteRemoteTransactionParameters),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct StringAttributeValue {
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Union)
)]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum PublicAttributeValue {
    String(StringAttributeValue),
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct PublicLocalSpanData {
    pub span_id: SpanId,
    pub start: Timestamp,
    pub parent_id: Option<SpanId>,
    pub linked_context: Option<u64>,
    pub attributes: Vec<PublicAttribute>,
    pub inherited: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct PublicAttribute {
    pub key: String,
    pub value: PublicAttributeValue,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct PublicExternalSpanData {
    pub span_id: SpanId,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Union)
)]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum PublicSpanData {
    LocalSpan(PublicLocalSpanData),
    ExternalSpan(PublicExternalSpanData),
}

impl PublicSpanData {
    pub fn span_id(&self) -> &SpanId {
        match self {
            PublicSpanData::LocalSpan(data) => &data.span_id,
            PublicSpanData::ExternalSpan(data) => &data.span_id,
        }
    }
}

/// API-facing representation of a named retry policy for public oplog entries.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct PublicNamedRetryPolicy {
    /// Human-readable identifier for this policy.
    pub name: String,
    /// Selection priority — higher values are evaluated first.
    pub priority: u32,
    /// The predicate that determines when this retry policy applies.
    pub predicate: ApiPredicate,
    /// The retry policy to use when the predicate matches.
    pub policy: ApiRetryPolicy,
}

#[cfg(feature = "full")]
impl From<crate::model::retry_policy::NamedRetryPolicy> for PublicNamedRetryPolicy {
    fn from(value: crate::model::retry_policy::NamedRetryPolicy) -> Self {
        Self {
            name: value.name,
            priority: value.priority,
            predicate: value.predicate.into(),
            policy: value.policy.into(),
        }
    }
}

#[cfg(feature = "full")]
impl From<PublicNamedRetryPolicy> for crate::model::retry_policy::NamedRetryPolicy {
    fn from(value: PublicNamedRetryPolicy) -> Self {
        Self {
            name: value.name,
            priority: value.priority,
            predicate: value.predicate.into(),
            policy: value.policy.into(),
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentInitializationParameters {
    pub idempotency_key: IdempotencyKey,
    pub constructor_parameters: TypedSchemaValue,
    pub trace_id: TraceId,
    pub trace_states: Vec<String>,
    pub invocation_context: Vec<Vec<PublicSpanData>>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentMethodInvocationParameters {
    pub idempotency_key: IdempotencyKey,
    pub method_name: String,
    pub function_input: TypedSchemaValue,
    pub trace_id: TraceId,
    pub trace_states: Vec<String>,
    pub invocation_context: Vec<Vec<PublicSpanData>>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct LoadSnapshotParameters {
    pub snapshot: PublicSnapshotData,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ProcessOplogEntriesParameters {
    pub idempotency_key: IdempotencyKey,
    // TODO
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ManualUpdateParameters {
    pub target_revision: ComponentRevision,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum PublicAgentInvocation {
    AgentInitialization(AgentInitializationParameters),
    AgentMethodInvocation(AgentMethodInvocationParameters),
    SaveSnapshot(Empty),
    LoadSnapshot(LoadSnapshotParameters),
    ProcessOplogEntries(ProcessOplogEntriesParameters),
    ManualUpdate(ManualUpdateParameters),
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct AgentInvocationOutputParameters {
    pub output: TypedSchemaValue,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct FallibleResultParameters {
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct SaveSnapshotResultParameters {
    pub snapshot: PublicSnapshotData,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct ProcessOplogEntriesResultParameters {
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum PublicAgentInvocationResult {
    AgentInitialization(AgentInvocationOutputParameters),
    AgentMethod(AgentInvocationOutputParameters),
    ManualUpdate(Empty),
    LoadSnapshot(FallibleResultParameters),
    SaveSnapshot(SaveSnapshotResultParameters),
    ProcessOplogEntries(ProcessOplogEntriesResultParameters),
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct SnapshotBasedUpdateParameters {
    pub payload: Vec<u8>,
    pub mime_type: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum PublicUpdateDescription {
    Automatic(Empty),
    SnapshotBased(SnapshotBasedUpdateParameters),
}

#[derive(Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::NewType)
)]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct AgentResourceId(pub u64);

impl AgentResourceId {
    pub const INITIAL: AgentResourceId = AgentResourceId(0);

    pub fn next(&self) -> AgentResourceId {
        AgentResourceId(self.0 + 1)
    }
}

impl Display for AgentResourceId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Worker log levels including the special stdout and stderr channels
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec, poem_openapi::Enum))]
#[repr(u8)]
pub enum LogLevel {
    #[cfg_attr(feature = "full", desert(transparent))]
    Stdout,
    #[cfg_attr(feature = "full", desert(transparent))]
    Stderr,
    #[cfg_attr(feature = "full", desert(transparent))]
    Trace,
    #[cfg_attr(feature = "full", desert(transparent))]
    Debug,
    #[cfg_attr(feature = "full", desert(transparent))]
    Info,
    #[cfg_attr(feature = "full", desert(transparent))]
    Warn,
    #[cfg_attr(feature = "full", desert(transparent))]
    Error,
    #[cfg_attr(feature = "full", desert(transparent))]
    Critical,
}

#[derive(Copy, Clone, Debug, PartialOrd, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec, poem_openapi::Enum))]
pub enum PersistenceLevel {
    PersistNothing,
    PersistRemoteSideEffects,
    Smart,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(
    feature = "full",
    derive(desert_rust::BinaryCodec, poem_openapi::Object)
)]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct RawSnapshotData {
    pub data: Vec<u8>,
    pub mime_type: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct JsonSnapshotData {
    pub data: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum MultipartPartData {
    Json(JsonSnapshotData),
    Raw(RawSnapshotData),
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct MultipartSnapshotPart {
    pub name: String,
    pub content_type: String,
    pub data: MultipartPartData,
}

#[derive(Clone, Debug, Serialize, PartialEq, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
pub struct MultipartSnapshotData {
    pub mime_type: String,
    pub parts: Vec<MultipartSnapshotPart>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum PublicSnapshotData {
    Raw(RawSnapshotData),
    Json(JsonSnapshotData),
    Multipart(MultipartSnapshotData),
}

/// API-facing counter state for a retry policy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[serde(rename_all = "camelCase")]
pub struct PublicRetryPolicyStateCounter {
    /// Number of retry attempts recorded so far.
    pub count: u32,
}

/// API-facing wrapper state that delegates to an inner retry policy state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[serde(rename_all = "camelCase")]
pub struct PublicRetryPolicyStateWrapper {
    /// The wrapped inner retry policy state.
    pub inner: Box<PublicRetryPolicyState>,
}

/// API-facing count-box state that tracks both an attempt count and an inner state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[serde(rename_all = "camelCase")]
pub struct PublicRetryPolicyStateCountBox {
    /// Number of attempts consumed so far.
    pub attempts: u32,
    /// The inner retry policy state.
    pub inner: Box<PublicRetryPolicyState>,
}

/// API-facing and-then state tracking left/right sub-states and which side is active.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[serde(rename_all = "camelCase")]
pub struct PublicRetryPolicyStateAndThen {
    /// State of the first (left) policy.
    pub left: Box<PublicRetryPolicyState>,
    /// State of the second (right) policy.
    pub right: Box<PublicRetryPolicyState>,
    /// Whether execution has moved to the right policy.
    pub on_right: bool,
}

/// API-facing pair state tracking two independent sub-states (for union/intersect).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[serde(rename_all = "camelCase")]
pub struct PublicRetryPolicyStatePair {
    /// State of the first sub-policy.
    pub first: Box<PublicRetryPolicyState>,
    /// State of the second sub-policy.
    pub second: Box<PublicRetryPolicyState>,
}

/// API-facing representation of a [`RetryPolicyState`](crate::model::retry_policy::RetryPolicyState),
/// exposed through the public REST/OpenAPI interface.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum PublicRetryPolicyState {
    /// Counter-based state (e.g. periodic, exponential).
    Counter(PublicRetryPolicyStateCounter),
    /// Terminal state — policy has given up.
    Terminal(Empty),
    /// Wrapper state delegating to an inner policy.
    Wrapper(PublicRetryPolicyStateWrapper),
    /// Count-box state with attempt tracking.
    CountBox(PublicRetryPolicyStateCountBox),
    /// And-then sequential composition state.
    AndThen(PublicRetryPolicyStateAndThen),
    /// Pair state for union/intersect composition.
    Pair(PublicRetryPolicyStatePair),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct QueuedCardEventCard {
    pub card_id: CardId,
    #[cfg_attr(feature = "full", oai(skip))]
    pub card: Option<StoredCard>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Object))]
#[cfg_attr(feature = "full", oai(rename_all = "camelCase"))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct PublicQueuedCardEventCard {
    pub card_id: CardId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum QueuedCardEvent {
    Install(QueuedCardEventCard),
    Revoke(QueuedCardEventCard),
}

impl QueuedCardEvent {
    pub fn card_id(&self) -> CardId {
        match self {
            Self::Install(event) | Self::Revoke(event) => event.card_id,
        }
    }

    pub fn install(card: impl Into<StoredCard>) -> Self {
        let card = card.into();
        Self::Install(QueuedCardEventCard {
            card_id: card.card_id(),
            card: Some(card),
        })
    }

    pub fn revoke(card_id: CardId) -> Self {
        Self::Revoke(QueuedCardEventCard {
            card_id,
            card: None,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "full", derive(poem_openapi::Union))]
#[cfg_attr(feature = "full", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum PublicQueuedCardEvent {
    Install(PublicQueuedCardEventCard),
    Revoke(PublicQueuedCardEventCard),
}

impl PublicQueuedCardEvent {
    pub fn card_id(&self) -> CardId {
        match self {
            Self::Install(event) | Self::Revoke(event) => event.card_id,
        }
    }
}

impl From<QueuedCardEvent> for PublicQueuedCardEvent {
    fn from(value: QueuedCardEvent) -> Self {
        match value {
            QueuedCardEvent::Install(event) => Self::Install(PublicQueuedCardEventCard {
                card_id: event.card_id,
            }),
            QueuedCardEvent::Revoke(event) => Self::Revoke(PublicQueuedCardEventCard {
                card_id: event.card_id,
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, IntoSchema, FromSchema)]
#[cfg_attr(feature = "full", derive(poem_openapi::Enum))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub enum CardInstallFailure {
    CardRevoked,
    NotFound,
    RecipientMismatch,
    NotPermitted,
}

#[cfg(feature = "full")]
impl From<crate::model::retry_policy::RetryPolicyState> for PublicRetryPolicyState {
    fn from(value: crate::model::retry_policy::RetryPolicyState) -> Self {
        use crate::model::retry_policy::RetryPolicyState;
        match value {
            RetryPolicyState::Counter(n) => {
                PublicRetryPolicyState::Counter(PublicRetryPolicyStateCounter { count: n })
            }
            RetryPolicyState::Terminal => PublicRetryPolicyState::Terminal(Empty {}),
            RetryPolicyState::Wrapper(inner) => {
                PublicRetryPolicyState::Wrapper(PublicRetryPolicyStateWrapper {
                    inner: Box::new((*inner).into()),
                })
            }
            RetryPolicyState::CountBox { attempts, inner } => {
                PublicRetryPolicyState::CountBox(PublicRetryPolicyStateCountBox {
                    attempts,
                    inner: Box::new((*inner).into()),
                })
            }
            RetryPolicyState::AndThen {
                left,
                right,
                on_right,
            } => PublicRetryPolicyState::AndThen(PublicRetryPolicyStateAndThen {
                left: Box::new((*left).into()),
                right: Box::new((*right).into()),
                on_right,
            }),
            RetryPolicyState::Pair(first, second) => {
                PublicRetryPolicyState::Pair(PublicRetryPolicyStatePair {
                    first: Box::new((*first).into()),
                    second: Box::new((*second).into()),
                })
            }
        }
    }
}

#[cfg(feature = "full")]
impl From<PublicRetryPolicyState> for crate::model::retry_policy::RetryPolicyState {
    fn from(value: PublicRetryPolicyState) -> Self {
        use crate::model::retry_policy::RetryPolicyState;
        match value {
            PublicRetryPolicyState::Counter(c) => RetryPolicyState::Counter(c.count),
            PublicRetryPolicyState::Terminal(_) => RetryPolicyState::Terminal,
            PublicRetryPolicyState::Wrapper(w) => {
                RetryPolicyState::Wrapper(Box::new((*w.inner).into()))
            }
            PublicRetryPolicyState::CountBox(cb) => RetryPolicyState::CountBox {
                attempts: cb.attempts,
                inner: Box::new((*cb.inner).into()),
            },
            PublicRetryPolicyState::AndThen(at) => RetryPolicyState::AndThen {
                left: Box::new((*at.left).into()),
                right: Box::new((*at.right).into()),
                on_right: at.on_right,
            },
            PublicRetryPolicyState::Pair(p) => {
                RetryPolicyState::Pair(Box::new((*p.first).into()), Box::new((*p.second).into()))
            }
        }
    }
}
