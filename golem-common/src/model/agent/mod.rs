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

mod agent_uuid;
mod config;
pub mod extraction;
mod invocation;
mod key;
mod method_input;
mod normalisation;
mod protobuf;
mod resolved_names;
mod status;

pub use key::AgentKey;
pub use method_input::{AgentMethodInput, JsonMethodInput, NativeMethodInput, PublicMethodInput};
pub use resolved_names::{
    FollowedLookups, MAX_RESOLVED_NAME_LINKS, OpenLookup, ResolvedNameIndex, ResolvedNames,
};

pub use invocation::{
    AgentInvocation, AgentInvocationKind, AgentInvocationOutput, AgentInvocationPayload,
    AgentInvocationResult, BackgroundFailure, InvocationRequestOperation, PendingInvocationRef,
    RedactedAgentInvocationResult, StoredAgentInvocationPayload, TimestampedAgentInvocation,
};
pub use status::{
    AgentMetadata, AgentStatusRecord, AssistedSelection, AuthoritativeSnapshot,
    AuthoritativeSnapshotKind, AutomaticSnapshot, DEFAULT_INVOCATION_RESULT_BLOOM_BITS,
    DEFAULT_INVOCATION_RESULT_BLOOM_HASHES, DEFAULT_RECENT_INVOCATION_RESULTS_CAPACITY,
    DURABLE_STREAM_SESSION_RECENT_CAPACITY, DurableStreamPublicBinding, DurableStreamSessionIndex,
    DurableStreamSessionStatus, EntityReplayBoundary, EntityReplayProgress, ExportForkAdmissions,
    ExportForkReservation, FailedUpdateRecord, InvocationEnd, InvocationOperation,
    InvocationRequestRef, InvocationResultBloom, InvocationResultMembership,
    InvocationResultPosition, MAX_INVOCATION_OPERATION_BYTES, OpenEntityReplayScope,
    OplogProcessorCheckpointState, PendingCardEventRef, PendingUpdateKind, PendingUpdateRef,
    RunningInvocation, SettlingInvocation, SnapshotFiles, SuccessfulUpdateRecord,
    UsableAutomaticSnapshot,
};

#[cfg(test)]
mod tests;

pub mod schema_evolution;
pub mod structural_format;
pub mod text_utils;

use std::fmt::{Display, Formatter};
use std::sync::LazyLock;
use std::time::Duration;

use base64::Engine;
use desert_rust::BinaryCodec;
use regex::Regex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::base_model::InvocationId;
pub use crate::base_model::agent::*;
pub use crate::base_model::worker::*;
use crate::model::account::AccountId;
use crate::model::component_metadata::ComponentMetadata;
use crate::model::environment::EnvironmentId;
use crate::model::oplog::OplogIndex;
use crate::model::{AgentFingerprint, AgentId, PromiseId, Timestamp};
use crate::schema::AgentTypeSchema;
pub use crate::schema::agent::ParsedAgentKey;
use crate::schema::graph::TypedSchemaValue;
use crate::schema::render::cli_text::value_to_cli_text;
use crate::schema::schema_type::SchemaType;
use crate::schema::schema_value::SchemaValue;

/// An invocation or creation address before it resolves to a concrete owner.
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum AgentTarget {
    /// The keyed agent that the key selects: its ID derives from the key.
    Key(AgentKey),
    /// The agent that has this ID.
    Id(AgentId),
}

/// Resolved execution-owner context. Consumers must branch on this value
/// instead of treating failure to parse an agent id as component-wide access.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedOwnerContext {
    Agent(Box<ParsedAgentKey>),
    /// An external-tool owner using component-level policy without constructing an agent.
    ComponentBaseline,
    OplogProcessor,
}

impl ResolvedOwnerContext {
    /// `agent` is the type and constructor parameters of a component agent, from its
    /// creation request or its Create entry. Other owner kinds have none.
    pub fn from_authoritative_kind(
        kind: OwnerKind,
        agent_id: &AgentId,
        agent: Option<ParsedAgentKey>,
        metadata: &ComponentMetadata,
    ) -> Result<Self, String> {
        kind.validate_agent_id(agent_id)?;
        match kind {
            OwnerKind::ComponentAgent if metadata.is_agent() => agent
                .map(Box::new)
                .map(Self::Agent)
                .ok_or_else(|| "A component agent has no agent type".to_string()),
            OwnerKind::ComponentAgent => Err("Component declares no agent types".to_string()),
            OwnerKind::EphemeralExternalTool | OwnerKind::StandaloneTool => {
                Ok(Self::ComponentBaseline)
            }
            OwnerKind::OplogProcessor if metadata.has_oplog_processor() => Ok(Self::OplogProcessor),
            OwnerKind::OplogProcessor => {
                Err("Component declares no oplog processor export".to_string())
            }
        }
    }

    pub fn agent(&self) -> Option<&ParsedAgentKey> {
        match self {
            Self::Agent(agent) => Some(agent),
            Self::ComponentBaseline | Self::OplogProcessor => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InvocationFreshnessDisposition {
    #[default]
    MayExist,
    KnownFresh,
}

impl TryFrom<i32> for AgentMode {
    type Error = String;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(AgentMode::Durable),
            1 => Ok(AgentMode::Ephemeral),
            _ => Err(format!("Unknown AgentMode: {value}")),
        }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[allow(clippy::large_enum_variant)]
pub enum AgentError {
    InvalidInput(String),
    InvalidMethod(String),
    InvalidType(String),
    InvalidAgentId(String),
    CustomError(TypedSchemaValue),
}

impl Display for AgentError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentError::InvalidInput(msg) => {
                write!(f, "Invalid input: {msg}")
            }
            AgentError::InvalidMethod(msg) => {
                write!(f, "Invalid method: {msg}")
            }
            AgentError::InvalidType(msg) => {
                write!(f, "Invalid type: {msg}")
            }
            AgentError::InvalidAgentId(msg) => {
                write!(f, "Invalid agent id: {msg}")
            }
            AgentError::CustomError(typed) => {
                let rendered = value_to_cli_text(typed.graph(), typed.root_type(), typed.value())
                    .unwrap_or_else(|_| "Unprintable error".to_string());
                write!(f, "{rendered}")
            }
        }
    }
}

impl Display for BinaryReference {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            BinaryReference::Url(url) => write!(f, "{url}"),
            BinaryReference::Inline(binary_source) => write!(f, "{binary_source}"),
        }
    }
}

impl Display for BinarySource {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}]\"{}\"",
            self.binary_type.mime_type,
            base64::engine::general_purpose::STANDARD.encode(&self.data)
        )
    }
}

static AGENT_KEY_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([^(]+)\((.*)\)$").expect("Invalid agent key regex"));

/// Parses the outer structure of agent key text, `Type(params)`, into the agent type
/// name and the parameter list.
pub(crate) fn parse_agent_key_parts(s: &str) -> Result<(&str, &str), String> {
    let captures = AGENT_KEY_REGEX.captures(s.trim()).ok_or_else(|| {
        format!("Unexpected agent key format - must be 'agent-type(...)', got: {s}")
    })?;

    let agent_type_name = captures.get(1).unwrap().as_str().trim();
    let param_list = captures.get(2).unwrap().as_str();

    if agent_type_name.is_empty() {
        return Err("Agent type name cannot be empty".to_string());
    }

    Ok((agent_type_name, param_list))
}

impl Display for Url {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.value)
    }
}

pub trait AgentTypeSchemaResolver {
    fn resolve_agent_type_schema_by_name(
        &self,
        agent_type: &AgentTypeName,
    ) -> Result<AgentTypeSchema, String>;
}

impl AgentTypeSchemaResolver for &ComponentMetadata {
    fn resolve_agent_type_schema_by_name(
        &self,
        agent_type: &AgentTypeName,
    ) -> Result<AgentTypeSchema, String> {
        self.find_agent_type_by_name(agent_type)
            .ok_or_else(|| format!("Agent type not found: {agent_type}"))
    }
}

impl AgentTypeSchemaResolver for &AgentTypeSchema {
    fn resolve_agent_type_schema_by_name(
        &self,
        agent_type: &AgentTypeName,
    ) -> Result<AgentTypeSchema, String> {
        if &self.type_name == agent_type {
            Ok((*self).clone())
        } else {
            Err(format!("Agent type not found: {agent_type}"))
        }
    }
}

impl ParsedAgentKey {
    /// Fails when the parameters have no canonical text.
    pub fn try_new(
        agent_type: AgentTypeName,
        parameters: TypedSchemaValue,
    ) -> Result<Self, String> {
        let constructor = Self::new(agent_type, parameters);
        constructor.canonical_parameters()?;
        Ok(constructor)
    }

    pub fn parse(
        s: impl AsRef<str>,
        resolver: impl AgentTypeSchemaResolver,
    ) -> Result<Self, String> {
        Self::parse_and_resolve_type(s, resolver).map(|(agent_id, _)| agent_id)
    }

    pub fn parse_and_resolve_type(
        s: impl AsRef<str>,
        resolver: impl AgentTypeSchemaResolver,
    ) -> Result<(Self, AgentTypeSchema), String> {
        let (key, agent_type) = AgentKey::parse(s.as_ref(), resolver)?;
        Ok((key.into(), agent_type))
    }

    /// Parses the constructor parameter text of an agent of the named type, as the
    /// Create entry records it.
    pub fn parse_parameters(
        agent_type_name: &AgentTypeName,
        parameters: &str,
        resolver: impl AgentTypeSchemaResolver,
    ) -> Result<Self, String> {
        use structural_format::{normalize_structural, parse_structural_typed};
        let agent_type = resolver.resolve_agent_type_schema_by_name(agent_type_name)?;
        let root = constructor_parameter_record(&agent_type);
        let value =
            parse_structural_typed(&normalize_structural(parameters), &agent_type.schema, &root)
                .map_err(|err| err.to_string())?;
        Self::try_new(
            agent_type_name.clone(),
            typed_constructor_parameters(&agent_type, value),
        )
    }

    pub fn parse_agent_type_name(s: &str) -> Result<AgentTypeName, String> {
        let (agent_type_name, _) = parse_agent_key_parts(s)?;
        Ok(AgentTypeName(agent_type_name.to_string()))
    }

    pub fn normalize_text(s: &str) -> Result<String, String> {
        normalisation::normalize_agent_key_text(s)
    }

    /// The canonical constructor parameter text, without the agent type name.
    pub fn canonical_parameters(&self) -> Result<String, String> {
        crate::model::agent::structural_format::format_structural_typed(&self.parameters)
            .map_err(|err| err.to_string())
    }

    /// Validates constructor parameters against the implementing component's schema,
    /// without imposing the length limit of a key.
    pub fn canonical_constructor(
        &self,
        resolver: impl AgentTypeSchemaResolver,
    ) -> Result<Self, String> {
        Self::parse_parameters(&self.agent_type, &self.canonical_parameters()?, resolver)
    }

    /// The key of this constructor, read back from its canonical parameter text against the
    /// constructor schema that `resolver` gives, so its parameters carry the implementing
    /// component's graph instead of the caller's. Each service that derives a keyed agent ID
    /// derives it from this key, so all of them reach the same ID.
    pub fn canonical_key(
        &self,
        resolver: impl AgentTypeSchemaResolver,
    ) -> Result<AgentKey, String> {
        let constructor = self.canonical_constructor(resolver)?;
        AgentKey::new(constructor.agent_type, constructor.parameters).map_err(|err| err.to_string())
    }
}

impl From<AgentKey> for ParsedAgentKey {
    fn from(key: AgentKey) -> Self {
        let (agent_type, parameters) = key.into_parts();
        Self::new(agent_type, parameters)
    }
}

impl Display for ParsedAgentKey {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        use crate::model::agent::structural_format::format_structural_typed;
        let rendered = format_structural_typed(&self.parameters).map_err(|_| std::fmt::Error)?;
        write!(f, "{}({rendered})", self.agent_type)
    }
}

pub fn typed_constructor_parameters(
    agent_type: &AgentTypeSchema,
    value: SchemaValue,
) -> TypedSchemaValue {
    let root = constructor_parameter_record(agent_type);
    // The result is a single-root carrier, so project the agent graph's defs to
    // exactly those reachable from this constructor record instead of cloning
    // the whole multi-root registry.
    crate::schema::agent::typed_schema_value_with_projected_defs(&agent_type.schema, root, value)
}

/// The record of a constructor's user-supplied parameters: the type of an agent key's
/// parameters. Agent config fields are not part of it.
pub fn constructor_parameter_record(agent_type: &AgentTypeSchema) -> SchemaType {
    use crate::schema::schema_type::NamedFieldType;

    SchemaType::record(
        agent_type
            .constructor
            .input_schema
            .fields()
            .iter()
            .map(|field| NamedFieldType {
                name: field.name.clone(),
                body: field.schema.clone(),
                metadata: field.metadata.clone(),
            })
            .collect(),
    )
}

/// Status of an idempotency key lookup on a worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, poem_openapi::Enum)]
#[serde(rename_all = "camelCase")]
#[oai(rename_all = "camelCase")]
pub enum InvocationStatus {
    /// The idempotency key is not known (never seen or expired).
    Unknown,
    /// The invocation is queued or currently executing.
    Pending,
    /// The invocation has completed (successfully or with an error).
    Complete,
    /// The invocation was cancelled before it started.
    Cancelled,
}

/// An agent ID with the environment that owns it
#[derive(
    Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize, BinaryCodec,
)]
#[desert(evolution())]
#[serde(rename_all = "camelCase")]
pub struct OwnedAgentId {
    pub environment_id: EnvironmentId,
    pub agent_id: AgentId,
}

impl OwnedAgentId {
    pub fn new(environment_id: EnvironmentId, agent_id: &AgentId) -> Self {
        Self {
            environment_id,
            agent_id: *agent_id,
        }
    }

    pub fn agent_id(&self) -> AgentId {
        self.agent_id
    }

    pub fn environment_id(&self) -> EnvironmentId {
        self.environment_id
    }

    /// The text key of the agent in every storage layer: `{env-uuid}/{agent-uuid}`.
    pub fn storage_key(&self) -> String {
        format!("{}/{}", self.environment_id, self.agent_id)
    }

    /// Reads a key that [`Self::storage_key`] wrote. Any other text, such as an upper-case or
    /// braced UUID, gives `None`.
    pub fn from_storage_key(key: &str) -> Option<Self> {
        let (environment_id, agent_id) = key.split_once('/')?;
        let owned_agent_id = Self {
            environment_id: environment_id.parse().ok()?,
            agent_id: agent_id.parse().ok()?,
        };
        (owned_agent_id.storage_key() == key).then_some(owned_agent_id)
    }

    /// The prefix that the storage keys of the agents of one type in the environment share, and
    /// no other key has: the environment, then the type tag, which is the first 8 hex digits of
    /// the agent ID.
    pub fn type_storage_key_prefix(
        environment_id: EnvironmentId,
        type_id: crate::model::type_id::TypeId,
    ) -> String {
        format!("{environment_id}/{:08x}-", type_id.get())
    }

    pub fn owner_id(&self) -> &OwnedAgentId {
        self
    }
}

impl Display for OwnedAgentId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.storage_key())
    }
}

impl AsRef<AgentId> for OwnedAgentId {
    fn as_ref(&self) -> &AgentId {
        &self.agent_id
    }
}

/// Actions that can be scheduled to be executed at a given point in time
#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub enum ScheduledAction {
    /// Completes a given promise
    CompletePromise {
        account_id: AccountId,
        environment_id: EnvironmentId,
        promise_id: PromiseId,
        target_worker_fingerprint: AgentFingerprint,
    },
    /// Archives all entries from the first non-empty layer of an oplog to the next layer,
    /// if the last oplog index did not change. If there are more layers below, schedules
    /// a next action to archive the next layer.
    #[desert(evolution(FieldAdded("agent_mode", AgentMode::Durable)))]
    ArchiveOplog {
        account_id: AccountId,
        owned_agent_id: OwnedAgentId,
        agent_mode: AgentMode,
        last_oplog_index: OplogIndex,
        next_after: Duration,
    },
    /// Wakes a scheduled invocation already committed in the target's oplog.
    /// Delivery requires the existing incarnation identified by `target_worker_fingerprint`.
    /// A missing agent or a fingerprint mismatch drops the action without creating an agent.
    WakeInvocation {
        owned_agent_id: OwnedAgentId,
        idempotency_key: InvocationId,
        target_worker_fingerprint: AgentFingerprint,
    },
    /// Resume the agent
    Resume {
        agent_created_by: AccountId,
        owned_agent_id: OwnedAgentId,
        target_worker_fingerprint: AgentFingerprint,
    },
    ExpireDurableStreamSession {
        owned_agent_id: OwnedAgentId,
        target_agent_fingerprint: AgentFingerprint,
        public_session_id: String,
        session_key: InvocationId,
        expected_deadline_millis: u64,
    },
}

impl ScheduledAction {
    pub fn owned_agent_id(&self) -> OwnedAgentId {
        match self {
            ScheduledAction::CompletePromise {
                environment_id,
                promise_id,
                ..
            } => OwnedAgentId::new(*environment_id, &promise_id.agent_id),
            ScheduledAction::ArchiveOplog { owned_agent_id, .. } => owned_agent_id.clone(),
            ScheduledAction::WakeInvocation { owned_agent_id, .. }
            | ScheduledAction::ExpireDurableStreamSession { owned_agent_id, .. } => {
                owned_agent_id.clone()
            }
            ScheduledAction::Resume { owned_agent_id, .. } => owned_agent_id.clone(),
        }
    }
}

impl Display for ScheduledAction {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ScheduledAction::CompletePromise { promise_id, .. } => {
                write!(f, "complete[{promise_id}]")
            }
            ScheduledAction::ArchiveOplog { owned_agent_id, .. } => {
                write!(f, "archive[{owned_agent_id}]")
            }
            ScheduledAction::WakeInvocation { owned_agent_id, .. } => {
                write!(f, "invoke[{owned_agent_id}]")
            }
            ScheduledAction::ExpireDurableStreamSession {
                owned_agent_id,
                public_session_id,
                ..
            } => write!(
                f,
                "expire-stream-session[{owned_agent_id}/{public_session_id}]"
            ),
            ScheduledAction::Resume { owned_agent_id, .. } => write!(f, "resume[{owned_agent_id}]"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, BinaryCodec)]
#[desert(evolution())]
pub struct ScheduleId {
    pub id: Uuid,
}

impl ScheduleId {
    pub fn fresh() -> Self {
        Self { id: Uuid::now_v7() }
    }

    pub fn for_invocation(fingerprint: AgentFingerprint, key: &InvocationId) -> Self {
        Self {
            id: Uuid::new_v5(&fingerprint.0, key.to_string().as_bytes()),
        }
    }
}

impl Display for ScheduleId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.id)
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, BinaryCodec, Serialize, Deserialize)]
#[desert(evolution())]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Critical,
}

impl Display for LogLevel {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            LogLevel::Trace => "trace",
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
            LogLevel::Critical => "critical",
        };
        write!(f, "{}", s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentEvent {
    /// `invocation_id` is the unique part of the invocation that wrote the output, or none when
    /// no invocation runs, such as during initialization or a snapshot.
    StdOut {
        timestamp: Timestamp,
        bytes: Vec<u8>,
        invocation_id: Option<InvocationId>,
    },
    StdErr {
        timestamp: Timestamp,
        bytes: Vec<u8>,
        invocation_id: Option<InvocationId>,
    },
    Log {
        timestamp: Timestamp,
        level: LogLevel,
        context: String,
        message: String,
        invocation_id: Option<InvocationId>,
    },
    InvocationStart {
        timestamp: Timestamp,
        function: String,
        invocation_id: InvocationId,
    },
    InvocationFinished {
        timestamp: Timestamp,
        function: String,
        invocation_id: InvocationId,
    },
    PluginError {
        timestamp: Timestamp,
        plugin_name: String,
        message: String,
    },
    SnapshotRecoverySucceeded {
        timestamp: Timestamp,
        snapshot_index: OplogIndex,
    },
    SnapshotRecoveryFailed {
        timestamp: Timestamp,
        snapshot_index: OplogIndex,
        error: String,
    },
    /// The client fell behind and the point it left of is no longer in our buffer.
    /// {number_of_skipped_messages} is the number of messages between the client left of and the point it is now at.
    ClientLagged { number_of_missed_messages: u64 },
}

impl Display for AgentEvent {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentEvent::StdOut { bytes, .. } => {
                write!(
                    f,
                    "<stdout> {}",
                    String::from_utf8(bytes.clone()).unwrap_or_default()
                )
            }
            AgentEvent::StdErr { bytes, .. } => {
                write!(
                    f,
                    "<stderr> {}",
                    String::from_utf8(bytes.clone()).unwrap_or_default()
                )
            }
            AgentEvent::Log {
                level,
                context,
                message,
                ..
            } => {
                write!(f, "<log> {level:?} {context} {message}")
            }
            AgentEvent::InvocationStart {
                function,
                invocation_id,
                ..
            } => {
                write!(f, "<invocation-start> {function} {invocation_id}")
            }
            AgentEvent::InvocationFinished {
                function,
                invocation_id,
                ..
            } => {
                write!(f, "<invocation-finished> {function} {invocation_id}")
            }
            AgentEvent::PluginError {
                plugin_name,
                message,
                ..
            } => {
                write!(f, "<plugin-error> [{plugin_name}] {message}")
            }
            AgentEvent::SnapshotRecoverySucceeded { snapshot_index, .. } => {
                write!(f, "<snapshot-recovery-succeeded> {snapshot_index}")
            }
            AgentEvent::SnapshotRecoveryFailed {
                snapshot_index,
                error,
                ..
            } => {
                write!(f, "<snapshot-recovery-failed> {snapshot_index}: {error}")
            }
            AgentEvent::ClientLagged {
                number_of_missed_messages,
            } => {
                write!(f, "<client-lagged> {number_of_missed_messages}")
            }
        }
    }
}
