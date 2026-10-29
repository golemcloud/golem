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

mod config;
pub mod extraction;
mod invocation;
mod normalisation;
mod protobuf;
mod status;

pub use invocation::{
    AgentInvocation, AgentInvocationKind, AgentInvocationOutput, AgentInvocationPayload,
    AgentInvocationResult, PendingInvocationRef, RedactedAgentInvocationResult,
    TimestampedAgentInvocation,
};
pub use status::{
    AgentMetadata, AgentStatusRecord, AssistedSelection, AuthoritativeSnapshot,
    AuthoritativeSnapshotKind, AutomaticSnapshot, DEFAULT_INVOCATION_RESULT_BLOOM_BITS,
    DEFAULT_INVOCATION_RESULT_BLOOM_HASHES, DEFAULT_RECENT_INVOCATION_RESULTS_CAPACITY,
    DURABLE_STREAM_SESSION_RECENT_CAPACITY, DurableStreamPublicBinding, DurableStreamSessionIndex,
    DurableStreamSessionStatus, EntityReplayBoundary, EntityReplayProgress, ExportForkAdmissions,
    ExportForkReservation, FailedUpdateRecord, InvocationResultBloom, InvocationResultMembership,
    OpenEntityReplayScope, OplogProcessorCheckpointState, PendingCardEventRef, PendingUpdateKind,
    PendingUpdateRef, SnapshotFiles, SuccessfulUpdateRecord, UsableAutomaticSnapshot,
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

use crate::base_model::IdempotencyKey;
pub use crate::base_model::agent::*;
pub use crate::base_model::worker::*;
use crate::model::account::AccountId;
use crate::model::component::{ComponentId, ComponentRevision};
use crate::model::component_metadata::{self, ComponentMetadata};
use crate::model::environment::EnvironmentId;
use crate::model::oplog::OplogIndex;
use crate::model::{AgentFingerprint, AgentId, PromiseId, Timestamp};
use crate::schema::AgentTypeSchema;
pub use crate::schema::agent::ParsedAgentId;
use crate::schema::graph::TypedSchemaValue;
use crate::schema::render::cli_text::value_to_cli_text;
use crate::schema::schema_type::SchemaType;
use crate::schema::schema_value::SchemaValue;

/// Resolved execution-owner context. Consumers must branch on this value
/// instead of treating failure to parse an agent id as component-wide access.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolvedOwnerContext {
    Agent(Box<ParsedAgentId>),
    /// A non-agent component's guest worker, using component-level provision policy.
    ComponentWorker,
    /// An ephemeral external-tool owner using component-level policy without constructing an agent.
    ComponentBaseline,
}

impl ResolvedOwnerContext {
    pub fn from_authoritative_kind(
        kind: OwnerKind,
        raw_agent_id: &str,
        metadata: &ComponentMetadata,
    ) -> Result<Self, String> {
        kind.validate_instance_name(raw_agent_id)?;
        match kind {
            OwnerKind::ComponentAgent if metadata.is_agent() => {
                ParsedAgentId::parse(raw_agent_id, metadata)
                    .map(Box::new)
                    .map(Self::Agent)
                    .map_err(|error| error.to_string())
            }
            OwnerKind::ComponentAgent => Ok(Self::ComponentWorker),
            OwnerKind::EphemeralExternalTool => Ok(Self::ComponentBaseline),
        }
    }

    pub fn agent(&self) -> Option<&ParsedAgentId> {
        match self {
            Self::Agent(agent) => Some(agent),
            Self::ComponentWorker | Self::ComponentBaseline => None,
        }
    }
}

const EPHEMERAL_INVOCATION_PHANTOM_NAMESPACE_V1: Uuid =
    uuid::uuid!("b6414d8d-acfb-4f13-9c5d-64a79af394e5");

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InvocationFreshnessDisposition {
    #[default]
    MayExist,
    KnownFresh,
}

pub fn ephemeral_invocation_phantom_id(idempotency_key: &IdempotencyKey) -> Uuid {
    Uuid::new_v5(
        &EPHEMERAL_INVOCATION_PHANTOM_NAMESPACE_V1,
        idempotency_key.value.as_bytes(),
    )
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

static AGENT_ID_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([^(]+)\((.*)\)(?:\[([^\]]+)\])?$").expect("Invalid agent ID regex")
});

/// Parses the outer structure of an agent ID string into its components:
/// (agent_type_name, param_list, optional_phantom_uuid_str)
pub(crate) fn parse_agent_id_parts(s: &str) -> Result<(&str, &str, Option<&str>), String> {
    let captures = AGENT_ID_REGEX.captures(s.trim()).ok_or_else(|| {
        format!("Unexpected agent-id format - must be 'agent-type(...)' or 'agent-type(...)[uuid]', got: {s}")
    })?;

    let agent_type_name = captures.get(1).unwrap().as_str().trim();
    let param_list = captures.get(2).unwrap().as_str();
    let phantom_id_str = captures.get(3).map(|m| m.as_str().trim());

    if agent_type_name.is_empty() {
        return Err("Agent type name cannot be empty".to_string());
    }

    Ok((agent_type_name, param_list, phantom_id_str))
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

impl ParsedAgentId {
    pub fn try_new(
        agent_type: AgentTypeName,
        parameters: TypedSchemaValue,
        phantom_id: Option<Uuid>,
    ) -> Result<Self, String> {
        let parsed = Self::new(agent_type, parameters, phantom_id);
        AgentId::validate_length(&parsed.to_string())?;
        Ok(parsed)
    }

    pub fn new_auto_phantom(
        agent_type: AgentTypeName,
        parameters: TypedSchemaValue,
        phantom_id: Option<Uuid>,
        mode: AgentMode,
    ) -> Result<Self, String> {
        let phantom_id = match (mode, phantom_id) {
            (_, Some(id)) => Some(id),
            (AgentMode::Ephemeral, None) => Some(Uuid::new_v4()),
            (AgentMode::Durable, None) => None,
        };
        Self::try_new(agent_type, parameters, phantom_id)
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
        use crate::model::agent::structural_format::{
            normalize_structural, parse_structural_typed,
        };
        use crate::schema::schema_type::NamedFieldType;

        let s = s.as_ref();
        let (agent_type_name, param_list, phantom_id_str) = parse_agent_id_parts(s)?;
        let agent_type_name = AgentTypeName(agent_type_name.to_string());
        let phantom_id = phantom_id_str
            .map(Uuid::parse_str)
            .transpose()
            .map_err(|e| format!("Invalid UUID in phantom ID: {e}"))?;
        let agent_type = resolver.resolve_agent_type_schema_by_name(&agent_type_name)?;
        let root = SchemaType::record(
            agent_type
                .constructor
                .input_schema
                .fields()
                .iter()
                .filter(|field| {
                    matches!(
                        field.source,
                        crate::schema::agent::FieldSource::UserSupplied
                    )
                })
                .map(|field| NamedFieldType {
                    name: field.name.clone(),
                    body: field.schema.clone(),
                    metadata: field.metadata.clone(),
                })
                .collect(),
        );
        let value =
            parse_structural_typed(&normalize_structural(param_list), &agent_type.schema, &root)
                .map_err(|e| e.to_string())?;
        let parameters = typed_constructor_parameters(&agent_type, value);
        let agent_id = Self::try_new(agent_type_name, parameters, phantom_id)?;
        Ok((agent_id, agent_type))
    }

    pub fn parse_agent_type_name(s: &str) -> Result<AgentTypeName, String> {
        let (agent_type_name, _, _) = parse_agent_id_parts(s)?;
        Ok(AgentTypeName(agent_type_name.to_string()))
    }

    pub fn normalize_text(s: &str) -> Result<String, String> {
        normalisation::normalize_agent_id_text(s)
    }

    pub fn with_phantom_id(&self, phantom_id: Option<Uuid>) -> Result<Self, String> {
        Self::try_new(self.agent_type.clone(), self.parameters.clone(), phantom_id)
    }

    pub fn with_ephemeral_invocation_phantom(
        &self,
        idempotency_key: &IdempotencyKey,
    ) -> Result<Self, String> {
        if self.phantom_id.is_some() {
            return Err("Cannot derive an ephemeral invocation identity from an agent ID that already has a phantom ID".to_string());
        }

        self.with_phantom_id(Some(ephemeral_invocation_phantom_id(idempotency_key)))
    }
}

impl Display for ParsedAgentId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        use crate::model::agent::structural_format::format_structural_typed;
        let rendered = format_structural_typed(&self.parameters).map_err(|_| std::fmt::Error)?;
        write!(f, "{}({rendered})", self.agent_type)?;
        if let Some(phantom_id) = self.phantom_id {
            write!(f, "[{phantom_id}]")?;
        }
        Ok(())
    }
}

pub fn typed_constructor_parameters(
    agent_type: &AgentTypeSchema,
    value: SchemaValue,
) -> TypedSchemaValue {
    use crate::schema::schema_type::NamedFieldType;

    let root = SchemaType::record(
        agent_type
            .constructor
            .input_schema
            .fields()
            .iter()
            .filter(|field| {
                matches!(
                    field.source,
                    crate::schema::agent::FieldSource::UserSupplied
                )
            })
            .map(|field| NamedFieldType {
                name: field.name.clone(),
                body: field.schema.clone(),
                metadata: field.metadata.clone(),
            })
            .collect(),
    );
    // The result is a single-root carrier, so project the agent graph's defs to
    // exactly those reachable from this constructor record instead of cloning
    // the whole multi-root registry.
    crate::schema::agent::typed_schema_value_with_projected_defs(&agent_type.schema, root, value)
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
}

impl AgentId {
    const AGENT_ID_MAX_LENGTH: usize = 512;

    pub fn validate_length(id: &str) -> Result<(), String> {
        if id.len() > Self::AGENT_ID_MAX_LENGTH {
            Err(format!(
                "Agent id is too long: {}, max length: {}, agent id: {}",
                id.len(),
                Self::AGENT_ID_MAX_LENGTH,
                id,
            ))
        } else {
            Ok(())
        }
    }

    pub fn from_agent_id(
        component_id: ComponentId,
        agent_id: &ParsedAgentId,
    ) -> Result<AgentId, String> {
        let agent_id = agent_id.to_string();
        Self::validate_length(&agent_id)?;
        Ok(Self {
            component_id,
            agent_id,
        })
    }

    pub fn from_agent_id_literal<S: AsRef<str>>(
        component_id: ComponentId,
        agent_id: S,
        resolver: impl AgentTypeSchemaResolver,
    ) -> Result<AgentId, String> {
        Self::from_agent_id(component_id, &ParsedAgentId::parse(agent_id, resolver)?)
    }

    pub fn from_component_metadata_and_agent_id<S: AsRef<str>>(
        component_id: ComponentId,
        component_metadata: &component_metadata::ComponentMetadata,
        id: S,
    ) -> Result<AgentId, String> {
        if component_metadata.is_agent() {
            Self::from_agent_id_literal(component_id, id, component_metadata)
        } else {
            let id = id.as_ref();
            if id.len() > Self::AGENT_ID_MAX_LENGTH {
                return Err(format!(
                    "Legacy worker id is too long: {}, max length: {}, worker id: {}",
                    id.len(),
                    Self::AGENT_ID_MAX_LENGTH,
                    id,
                ));
            }
            if id.contains('/') {
                return Err(format!(
                    "Legacy worker id cannot contain '/', worker id: {}",
                    id,
                ));
            }

            Ok(AgentId {
                component_id,
                agent_id: id.to_string(),
            })
        }
    }

    pub fn from_agent_name_string<S: AsRef<str>>(
        component_id: ComponentId,
        id: S,
    ) -> Result<AgentId, String> {
        let id = id.as_ref();

        match ParsedAgentId::normalize_text(id) {
            Ok(normalized) => {
                if normalized.len() > Self::AGENT_ID_MAX_LENGTH {
                    return Err(format!(
                        "Agent id is too long: {}, max length: {}, agent id: {}",
                        normalized.len(),
                        Self::AGENT_ID_MAX_LENGTH,
                        normalized,
                    ));
                }
                Ok(AgentId {
                    component_id,
                    agent_id: normalized,
                })
            }
            Err(_) => {
                if id.len() > Self::AGENT_ID_MAX_LENGTH {
                    return Err(format!(
                        "Legacy worker id is too long: {}, max length: {}, worker id: {}",
                        id.len(),
                        Self::AGENT_ID_MAX_LENGTH,
                        id,
                    ));
                }
                if id.contains('/') {
                    return Err(format!(
                        "Legacy worker id cannot contain '/', worker id: {}",
                        id,
                    ));
                }
                Ok(AgentId {
                    component_id,
                    agent_id: id.to_string(),
                })
            }
        }
    }
}

/// Associates an agent-id with its owner project
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
            agent_id: agent_id.clone(),
        }
    }

    pub fn agent_id(&self) -> AgentId {
        self.agent_id.clone()
    }

    pub fn environment_id(&self) -> EnvironmentId {
        self.environment_id
    }

    pub fn component_id(&self) -> ComponentId {
        self.agent_id.component_id
    }

    pub fn agent_name(&self) -> String {
        self.agent_id.agent_id.clone()
    }

    pub fn owner_id(&self) -> &OwnedAgentId {
        self
    }
}

impl Display for OwnedAgentId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.environment_id(), self.agent_id)
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
    /// Invoke the given action on the worker. The invocation will only
    /// be persisted in the oplog when it's actually getting scheduled.
    ///
    /// `target_worker_fingerprint` guards against stale invocations: when `Some(fp)`, the
    /// scheduler compares `fp` against the current worker's `fingerprint` before firing. A
    /// mismatch means the original worker was deleted and a new one was created with the same
    /// ID — the invocation is silently dropped. `None` means fire unconditionally (used for
    /// legacy entries and non-wasm-rpc invocations).
    Invoke {
        account_id: AccountId,
        owned_agent_id: OwnedAgentId,
        invocation: Box<AgentInvocation>,
        target_worker_fingerprint: AgentFingerprint,
    },
    /// Resume the agent
    Resume {
        agent_created_by: AccountId,
        owned_agent_id: OwnedAgentId,
    },
    /// Invokes a fresh ephemeral agent without pre-creating it. The scheduler uses the checked
    /// creation path because delivery may be retried after an ambiguous failure.
    InvokeEphemeral {
        account_id: AccountId,
        owned_agent_id: OwnedAgentId,
        invocation: Box<AgentInvocation>,
        component_revision: ComponentRevision,
        env: Vec<(String, String)>,
        config: Vec<AgentConfigEntryDto>,
        parent: Option<AgentId>,
        creation_principal: Box<Principal>,
    },
    ExpireDurableStreamSession {
        owned_agent_id: OwnedAgentId,
        target_agent_fingerprint: AgentFingerprint,
        public_session_id: String,
        session_key: IdempotencyKey,
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
            ScheduledAction::Invoke { owned_agent_id, .. }
            | ScheduledAction::InvokeEphemeral { owned_agent_id, .. }
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
            ScheduledAction::Invoke { owned_agent_id, .. }
            | ScheduledAction::InvokeEphemeral { owned_agent_id, .. } => {
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

    pub fn from_idempotency_key(key: &IdempotencyKey) -> Self {
        Self {
            id: Uuid::parse_str(&key.value).expect("derived idempotency key must be a UUID"),
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
    StdOut {
        timestamp: Timestamp,
        bytes: Vec<u8>,
    },
    StdErr {
        timestamp: Timestamp,
        bytes: Vec<u8>,
    },
    Log {
        timestamp: Timestamp,
        level: LogLevel,
        context: String,
        message: String,
    },
    InvocationStart {
        timestamp: Timestamp,
        function: String,
        idempotency_key: IdempotencyKey,
    },
    InvocationFinished {
        timestamp: Timestamp,
        function: String,
        idempotency_key: IdempotencyKey,
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
                idempotency_key,
                ..
            } => {
                write!(f, "<invocation-start> {function} {idempotency_key}")
            }
            AgentEvent::InvocationFinished {
                function,
                idempotency_key,
                ..
            } => {
                write!(f, "<invocation-finished> {function} {idempotency_key}")
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
