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

use desert_rust::BinaryCodec;

use super::{InvocationStatus, Principal};
use crate::model::account::AccountId;
use crate::model::card::ScopeCard;
use crate::model::component::ComponentRevision;
use crate::model::invocation_context::InvocationContextStack;
use crate::model::oplog::payload::types::{
    SerializableToolError, SerializableToolInvocationResult, SerializableToolRpcError,
};
use crate::model::oplog::types::AgentMetadataForGuests;
use crate::model::oplog::{OplogEntry, OplogIndex, RawSnapshotData};
use crate::model::tool::{ToolActivationSnapshot, ToolName};
use crate::model::{AgentFingerprint, AgentId, InvocationId, Timestamp};
use crate::schema::{ResultValuePayload, SchemaValue, TypedSchemaValue};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentInvocationKind {
    AgentInitialization,
    AgentMethod,
    ManualUpdate,
    LoadSnapshot,
    SaveSnapshot,
    ProcessOplogEntries,
    ExternalTool,
}

#[derive(Clone, Debug, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub enum AgentInvocation {
    ManualUpdate {
        target_revision: ComponentRevision,
    },
    AgentInitialization {
        idempotency_key: InvocationId,
        input: SchemaValue,
        invocation_context: InvocationContextStack,
        principal: Principal,
    },
    AgentMethod {
        idempotency_key: InvocationId,
        method_name: String,
        input: SchemaValue,
        invocation_context: InvocationContextStack,
        principal: Principal,
        scope_card: Option<ScopeCard>,
    },
    ExternalTool {
        idempotency_key: InvocationId,
        tool_name: ToolName,
        command_path: Vec<String>,
        input: Box<TypedSchemaValue>,
        stdin: bool,
        stdout: bool,
        stderr: bool,
        activation: Box<ToolActivationSnapshot>,
        invocation_context: InvocationContextStack,
        principal: Principal,
        scope_card: Option<ScopeCard>,
    },
    LoadSnapshot {
        idempotency_key: InvocationId,
        snapshot: RawSnapshotData, // TODO
    },
    SaveSnapshot {
        idempotency_key: InvocationId,
        // TODO
    },
    ProcessOplogEntries {
        idempotency_key: InvocationId,
        account_id: AccountId,
        config: Vec<(String, String)>,
        metadata: AgentMetadataForGuests,
        first_entry_index: OplogIndex,
        entries: Vec<OplogEntry>,
    },
}

#[derive(Clone, Debug, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub enum AgentInvocationPayload {
    ManualUpdate {
        target_revision: ComponentRevision,
    },
    AgentInitialization {
        input: SchemaValue,
        principal: Principal,
    },
    AgentMethod {
        method_name: String,
        input: SchemaValue,
        principal: Principal,
        scope_card: Option<ScopeCard>,
    },
    ExternalTool {
        tool_name: ToolName,
        command_path: Vec<String>,
        input: Box<TypedSchemaValue>,
        stdin: bool,
        stdout: bool,
        stderr: bool,
        activation: Box<ToolActivationSnapshot>,
        principal: Principal,
        scope_card: Option<ScopeCard>,
    },
    LoadSnapshot {
        snapshot: RawSnapshotData,
    },
    SaveSnapshot,
    ProcessOplogEntries {
        account_id: AccountId,
        config: Vec<(String, String)>,
        metadata: AgentMetadataForGuests,
        first_entry_index: OplogIndex,
        entries: Vec<crate::model::oplog::OplogEntry>,
    },
}

impl AgentInvocationPayload {
    /// Compares the caller's execution contract, not deployment-time tool resolution.
    /// Tracing and scheduling metadata are stored outside the payload.
    pub fn same_caller_request(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::AgentInitialization { input, principal },
                Self::AgentInitialization {
                    input: other_input,
                    principal: other_principal,
                },
            ) => principal == other_principal && schema_value_same_request(input, other_input),
            (
                Self::AgentMethod {
                    method_name,
                    input,
                    principal,
                    scope_card,
                },
                Self::AgentMethod {
                    method_name: other_method,
                    input: other_input,
                    principal: other_principal,
                    scope_card: other_card,
                },
            ) => {
                method_name == other_method
                    && principal == other_principal
                    && scope_card == other_card
                    && schema_value_same_request(input, other_input)
            }
            (
                Self::ExternalTool {
                    tool_name,
                    command_path,
                    input,
                    stdin,
                    stdout,
                    stderr,
                    principal,
                    scope_card,
                    ..
                },
                Self::ExternalTool {
                    tool_name: other_tool,
                    command_path: other_path,
                    input: other_input,
                    stdin: other_stdin,
                    stdout: other_stdout,
                    stderr: other_stderr,
                    principal: other_principal,
                    scope_card: other_card,
                    ..
                },
            ) => {
                tool_name == other_tool
                    && command_path == other_path
                    && input.graph() == other_input.graph()
                    && schema_value_same_request(input.value(), other_input.value())
                    && stdin == other_stdin
                    && stdout == other_stdout
                    && stderr == other_stderr
                    && principal == other_principal
                    && scope_card == other_card
            }
            _ => self == other,
        }
    }
}

/// Compares stored request values without losing NaN payloads or the sign of zero.
/// Non-float leaves, including stream identities and capabilities, retain their equality.
fn schema_value_same_request(a: &SchemaValue, b: &SchemaValue) -> bool {
    fn opt_same(a: &Option<Box<SchemaValue>>, b: &Option<Box<SchemaValue>>) -> bool {
        match (a, b) {
            (Some(a), Some(b)) => schema_value_same_request(a, b),
            (None, None) => true,
            _ => false,
        }
    }

    match (a, b) {
        (SchemaValue::F32(a), SchemaValue::F32(b)) => a.to_bits() == b.to_bits(),
        (SchemaValue::F64(a), SchemaValue::F64(b)) => a.to_bits() == b.to_bits(),
        (SchemaValue::Record { fields: a }, SchemaValue::Record { fields: b })
        | (SchemaValue::Tuple { elements: a }, SchemaValue::Tuple { elements: b })
        | (SchemaValue::List { elements: a }, SchemaValue::List { elements: b })
        | (SchemaValue::FixedList { elements: a }, SchemaValue::FixedList { elements: b }) => {
            a.len() == b.len()
                && a.iter()
                    .zip(b)
                    .all(|(a, b)| schema_value_same_request(a, b))
        }
        (SchemaValue::Map { entries: a }, SchemaValue::Map { entries: b }) => {
            a.len() == b.len()
                && a.iter().zip(b).all(|((ak, av), (bk, bv))| {
                    schema_value_same_request(ak, bk) && schema_value_same_request(av, bv)
                })
        }
        (SchemaValue::Variant(a), SchemaValue::Variant(b)) => {
            a.case == b.case && opt_same(&a.payload, &b.payload)
        }
        (SchemaValue::Option { inner: a }, SchemaValue::Option { inner: b }) => opt_same(a, b),
        (SchemaValue::Result(a), SchemaValue::Result(b)) => match (a, b) {
            (ResultValuePayload::Ok { value: a }, ResultValuePayload::Ok { value: b })
            | (ResultValuePayload::Err { value: a }, ResultValuePayload::Err { value: b }) => {
                opt_same(a, b)
            }
            _ => false,
        },
        (SchemaValue::Union(a), SchemaValue::Union(b)) => {
            a.tag == b.tag && schema_value_same_request(&a.body, &b.body)
        }
        _ => a == b,
    }
}

#[derive(Clone, Debug, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub enum AgentInvocationResult {
    AgentInitialization,
    AgentMethod {
        output: SchemaValue,
    },
    ManualUpdate,
    LoadSnapshot {
        error: Option<String>,
    },
    SaveSnapshot {
        snapshot: RawSnapshotData,
    },
    ProcessOplogEntries {
        error: Option<String>,
    },
    ExternalTool {
        result: Result<SerializableToolInvocationResult, SerializableToolRpcError>,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentInvocationOutput {
    pub result: AgentInvocationResult,
    pub consumed_fuel: Option<u64>,
    pub invocation_status: Option<InvocationStatus>,
    pub component_revision: Option<ComponentRevision>,
    /// Final target of this invocation. Ephemeral invocations include their
    /// generated one-shot phantom ID. `None` when produced by a legacy executor
    /// or within the worker runtime before transport metadata is attached.
    pub agent_id: Option<AgentId>,
    /// Final idempotency key for this invocation. `None` when produced by a
    /// legacy executor or within the worker runtime before transport metadata
    /// is attached.
    pub idempotency_key: Option<InvocationId>,
    /// Oplog index of the agent right after this invocation completed. `None`
    /// for synthetic outputs (e.g. lookup-only status responses) or for legacy
    /// executors that did not report it.
    pub oplog_index: Option<OplogIndex>,
    /// Per-instance fingerprint of the agent that produced this invocation
    /// result. `None` for synthetic outputs (e.g. lookup-only status responses)
    /// or for legacy executors that did not report it.
    pub agent_fingerprint: Option<AgentFingerprint>,
}

/// Compares two schema-native values for replay equivalence. This is the same
/// as structural equality except that `NaN` floats compare equal (so that a
/// replayed invocation result carrying a `NaN` is not spuriously reported as
/// diverging).
fn schema_value_replay_equivalent(a: &SchemaValue, b: &SchemaValue) -> bool {
    fn opt_box_equiv(a: &Option<Box<SchemaValue>>, b: &Option<Box<SchemaValue>>) -> bool {
        match (a, b) {
            (Some(a), Some(b)) => schema_value_replay_equivalent(a, b),
            (None, None) => true,
            _ => false,
        }
    }

    fn slice_equiv(xs: &[SchemaValue], ys: &[SchemaValue]) -> bool {
        xs.len() == ys.len()
            && xs
                .iter()
                .zip(ys.iter())
                .all(|(x, y)| schema_value_replay_equivalent(x, y))
    }

    match (a, b) {
        (SchemaValue::F32(x), SchemaValue::F32(y)) => (x.is_nan() && y.is_nan()) || x == y,
        (SchemaValue::F64(x), SchemaValue::F64(y)) => (x.is_nan() && y.is_nan()) || x == y,
        (SchemaValue::Record { fields: xs }, SchemaValue::Record { fields: ys })
        | (SchemaValue::Tuple { elements: xs }, SchemaValue::Tuple { elements: ys })
        | (SchemaValue::List { elements: xs }, SchemaValue::List { elements: ys })
        | (SchemaValue::FixedList { elements: xs }, SchemaValue::FixedList { elements: ys }) => {
            slice_equiv(xs, ys)
        }
        (SchemaValue::Map { entries: xs }, SchemaValue::Map { entries: ys }) => {
            xs.len() == ys.len()
                && xs.iter().zip(ys.iter()).all(|((xk, xv), (yk, yv))| {
                    schema_value_replay_equivalent(xk, yk) && schema_value_replay_equivalent(xv, yv)
                })
        }
        (SchemaValue::Variant(a), SchemaValue::Variant(b)) => {
            a.case == b.case && opt_box_equiv(&a.payload, &b.payload)
        }
        (SchemaValue::Option { inner: a }, SchemaValue::Option { inner: b }) => opt_box_equiv(a, b),
        (SchemaValue::Result(a), SchemaValue::Result(b)) => match (a, b) {
            (ResultValuePayload::Ok { value: a }, ResultValuePayload::Ok { value: b })
            | (ResultValuePayload::Err { value: a }, ResultValuePayload::Err { value: b }) => {
                opt_box_equiv(a, b)
            }
            _ => false,
        },
        (SchemaValue::Union(a), SchemaValue::Union(b)) => {
            a.tag == b.tag && schema_value_replay_equivalent(&a.body, &b.body)
        }
        _ => a == b,
    }
}

impl AgentInvocationResult {
    pub fn replay_equivalent(&self, other: &AgentInvocationResult) -> bool {
        match (self, other) {
            (
                AgentInvocationResult::AgentInitialization,
                AgentInvocationResult::AgentInitialization,
            ) => true,
            (
                AgentInvocationResult::AgentMethod { output: a },
                AgentInvocationResult::AgentMethod { output: b },
            ) => schema_value_replay_equivalent(a, b),
            (AgentInvocationResult::ManualUpdate, AgentInvocationResult::ManualUpdate) => true,
            (
                AgentInvocationResult::LoadSnapshot { error: a },
                AgentInvocationResult::LoadSnapshot { error: b },
            ) => a == b,
            (
                AgentInvocationResult::SaveSnapshot { snapshot: a },
                AgentInvocationResult::SaveSnapshot { snapshot: b },
            ) => a == b,
            (
                AgentInvocationResult::ProcessOplogEntries { error: a },
                AgentInvocationResult::ProcessOplogEntries { error: b },
            ) => a == b,
            (
                AgentInvocationResult::ExternalTool { result: a },
                AgentInvocationResult::ExternalTool { result: b },
            ) => match (a, b) {
                (Ok(a), Ok(b)) => match (&a.result, &b.result) {
                    (Some(a), Some(b)) => {
                        a.graph() == b.graph()
                            && schema_value_replay_equivalent(a.value(), b.value())
                    }
                    (None, None) => true,
                    _ => false,
                },
                (
                    Err(SerializableToolRpcError::RemoteToolError(a)),
                    Err(SerializableToolRpcError::RemoteToolError(b)),
                ) => match (a.as_ref(), b.as_ref()) {
                    (
                        SerializableToolError::CustomError(a),
                        SerializableToolError::CustomError(b),
                    ) => {
                        a.name == b.name
                            && a.payload.graph() == b.payload.graph()
                            && schema_value_replay_equivalent(a.payload.value(), b.payload.value())
                    }
                    _ => a == b,
                },
                (Err(a), Err(b)) => a == b,
                _ => false,
            },
            _ => false,
        }
    }

    /// Wraps this result for `Debug` rendering that redacts any host-managed
    /// capability material (`Secret` / `QuotaToken`) in the embedded
    /// invocation output. Use at tracing / diagnostic / error-formatting
    /// boundaries instead of the derived `Debug`.
    pub fn redacted_debug(&self) -> RedactedAgentInvocationResult<'_> {
        RedactedAgentInvocationResult(self)
    }
}

/// `Debug` wrapper produced by [`AgentInvocationResult::redacted_debug`].
pub struct RedactedAgentInvocationResult<'a>(&'a AgentInvocationResult);

impl std::fmt::Debug for RedactedAgentInvocationResult<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.0 {
            AgentInvocationResult::AgentMethod { output } => f
                .debug_struct("AgentMethod")
                .field(
                    "output",
                    &crate::schema::redacted_schema_value_debug(output),
                )
                .finish(),
            AgentInvocationResult::ExternalTool { result } => {
                let mut debug = f.debug_struct("ExternalTool");
                match result {
                    Ok(result) => match &result.result {
                        Some(value) => debug.field(
                            "result",
                            &format_args!(
                                "Ok({:?})",
                                crate::schema::redact_host_managed_typed_value((**value).clone())
                            ),
                        ),
                        None => debug.field("result", &"Ok(None)"),
                    },
                    Err(SerializableToolRpcError::RemoteToolError(error)) => match error.as_ref() {
                        crate::base_model::tool::SerializableToolError::CustomError(value) => debug
                            .field(
                                "result",
                                &format_args!(
                                    "Err(RemoteToolError(CustomError {{ name: {:?}, payload: {:?} }}))",
                                    value.name,
                                    crate::schema::redact_host_managed_typed_value(value.payload.clone())
                                ),
                            ),
                        error => {
                            debug.field("result", &format_args!("Err(RemoteToolError({error:?}))"))
                        }
                    },
                    Err(error) => debug.field("result", &format_args!("Err({error:?})")),
                };
                debug.finish()
            }
            other => std::fmt::Debug::fmt(other, f),
        }
    }
}

impl AgentInvocation {
    pub fn from_parts(
        idempotency_key: InvocationId,
        payload: AgentInvocationPayload,
        invocation_context: InvocationContextStack,
    ) -> Self {
        match payload {
            AgentInvocationPayload::ManualUpdate { target_revision } => {
                Self::ManualUpdate { target_revision }
            }
            AgentInvocationPayload::AgentInitialization { input, principal } => {
                Self::AgentInitialization {
                    idempotency_key,
                    input,
                    invocation_context,
                    principal,
                }
            }
            AgentInvocationPayload::AgentMethod {
                method_name,
                input,
                principal,
                scope_card,
            } => Self::AgentMethod {
                idempotency_key,
                method_name,
                input,
                invocation_context,
                principal,
                scope_card,
            },
            AgentInvocationPayload::ExternalTool {
                tool_name,
                command_path,
                input,
                stdin,
                stdout,
                stderr,
                activation,
                principal,
                scope_card,
            } => Self::ExternalTool {
                idempotency_key,
                tool_name,
                command_path,
                input,
                stdin,
                stdout,
                stderr,
                activation,
                invocation_context,
                principal,
                scope_card,
            },
            AgentInvocationPayload::LoadSnapshot { snapshot } => Self::LoadSnapshot {
                idempotency_key,
                snapshot,
            },
            AgentInvocationPayload::SaveSnapshot => Self::SaveSnapshot { idempotency_key },
            AgentInvocationPayload::ProcessOplogEntries {
                account_id,
                config,
                metadata,
                first_entry_index,
                entries,
            } => Self::ProcessOplogEntries {
                idempotency_key,
                account_id,
                config,
                metadata,
                first_entry_index,
                entries,
            },
        }
    }

    pub fn into_parts(self) -> (InvocationId, AgentInvocationPayload, InvocationContextStack) {
        match self {
            Self::ManualUpdate { target_revision } => (
                InvocationId(Uuid::new_v4()),
                AgentInvocationPayload::ManualUpdate { target_revision },
                InvocationContextStack::fresh(),
            ),
            Self::AgentInitialization {
                idempotency_key,
                input,
                invocation_context,
                principal,
            } => (
                idempotency_key,
                AgentInvocationPayload::AgentInitialization { input, principal },
                invocation_context,
            ),
            Self::AgentMethod {
                idempotency_key,
                method_name,
                input,
                invocation_context,
                principal,
                scope_card,
            } => (
                idempotency_key,
                AgentInvocationPayload::AgentMethod {
                    method_name,
                    input,
                    principal,
                    scope_card,
                },
                invocation_context,
            ),
            Self::ExternalTool {
                idempotency_key,
                tool_name,
                command_path,
                input,
                stdin,
                stdout,
                stderr,
                activation,
                invocation_context,
                principal,
                scope_card,
                ..
            } => (
                idempotency_key,
                AgentInvocationPayload::ExternalTool {
                    tool_name,
                    command_path,
                    input,
                    stdin,
                    stdout,
                    stderr,
                    activation,
                    principal,
                    scope_card,
                },
                invocation_context,
            ),
            Self::LoadSnapshot {
                idempotency_key,
                snapshot,
            } => (
                idempotency_key,
                AgentInvocationPayload::LoadSnapshot { snapshot },
                InvocationContextStack::fresh(),
            ),
            Self::SaveSnapshot { idempotency_key } => (
                idempotency_key,
                AgentInvocationPayload::SaveSnapshot,
                InvocationContextStack::fresh(),
            ),
            Self::ProcessOplogEntries {
                idempotency_key,
                account_id,
                config,
                metadata,
                first_entry_index,
                entries,
            } => (
                idempotency_key,
                AgentInvocationPayload::ProcessOplogEntries {
                    account_id,
                    config,
                    metadata,
                    first_entry_index,
                    entries,
                },
                InvocationContextStack::fresh(),
            ),
        }
    }

    pub fn has_idempotency_key(&self, key: &InvocationId) -> bool {
        self.idempotency_key() == Some(key)
    }

    pub fn idempotency_key(&self) -> Option<&InvocationId> {
        match self {
            Self::AgentMethod {
                idempotency_key, ..
            } => Some(idempotency_key),
            Self::ExternalTool {
                idempotency_key, ..
            } => Some(idempotency_key),
            Self::AgentInitialization {
                idempotency_key, ..
            } => Some(idempotency_key),
            Self::ProcessOplogEntries {
                idempotency_key, ..
            } => Some(idempotency_key),
            Self::SaveSnapshot { idempotency_key } => Some(idempotency_key),
            Self::LoadSnapshot {
                idempotency_key, ..
            } => Some(idempotency_key),
            _ => None,
        }
    }

    pub fn invocation_context(&self) -> InvocationContextStack {
        match self {
            Self::AgentInitialization {
                invocation_context, ..
            } => invocation_context.clone(),
            Self::AgentMethod {
                invocation_context, ..
            } => invocation_context.clone(),
            Self::ExternalTool {
                invocation_context, ..
            } => invocation_context.clone(),
            _ => InvocationContextStack::fresh(),
        }
    }

    pub fn kind(&self) -> AgentInvocationKind {
        match self {
            Self::ManualUpdate { .. } => AgentInvocationKind::ManualUpdate,
            Self::AgentInitialization { .. } => AgentInvocationKind::AgentInitialization,
            Self::AgentMethod { .. } => AgentInvocationKind::AgentMethod,
            Self::ExternalTool { .. } => AgentInvocationKind::ExternalTool,
            Self::LoadSnapshot { .. } => AgentInvocationKind::LoadSnapshot,
            Self::SaveSnapshot { .. } => AgentInvocationKind::SaveSnapshot,
            Self::ProcessOplogEntries { .. } => AgentInvocationKind::ProcessOplogEntries,
        }
    }

    pub fn display_name(&self) -> String {
        match self {
            Self::ManualUpdate { .. } => String::new(),
            Self::AgentInitialization { .. } => "initialize".to_string(),
            Self::AgentMethod { method_name, .. } => method_name.clone(),
            Self::ExternalTool {
                tool_name,
                command_path,
                ..
            } => format!("{tool_name}:{}", command_path.join("/")),
            Self::LoadSnapshot { .. } => "load-snapshot".to_string(),
            Self::SaveSnapshot { .. } => "save-snapshot".to_string(),
            Self::ProcessOplogEntries { .. } => "process-oplog-entries".to_string(),
        }
    }

    pub fn principal(&self) -> Option<&Principal> {
        match self {
            Self::AgentInitialization { principal, .. }
            | Self::AgentMethod { principal, .. }
            | Self::ExternalTool { principal, .. } => Some(principal),
            _ => None,
        }
    }

    pub fn scope_card(&self) -> Option<&ScopeCard> {
        match self {
            Self::AgentMethod { scope_card, .. } | Self::ExternalTool { scope_card, .. } => {
                scope_card.as_ref()
            }
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct TimestampedAgentInvocation {
    pub timestamp: Timestamp,
    pub invocation: AgentInvocation,
}

/// A lightweight reference to a pending agent invocation whose full payload is stored in the
/// oplog.
///
/// The complete invocation (input parameters, snapshot data, oplog entry batches, ...) lives
/// in the `PendingAgentInvocation` oplog entry at `oplog_index`. The status record only keeps
/// the minimal routing metadata that consumers need without executing the invocation. Paths
/// that actually run the invocation hydrate the full [`TimestampedAgentInvocation`] from the
/// oplog on demand.
#[derive(Clone, Debug, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct PendingInvocationRef {
    pub timestamp: Timestamp,
    pub not_before: Option<Timestamp>,
    /// Index of the `PendingAgentInvocation` oplog entry holding the full payload.
    pub oplog_index: OplogIndex,
    /// Semantic idempotency key of the invocation. `None` for manual updates.
    pub idempotency_key: Option<InvocationId>,
    /// Target revision of the manual update. `Some` only for manual update invocations.
    pub manual_update_target_revision: Option<ComponentRevision>,
}

impl PendingInvocationRef {
    pub fn is_due(&self, now: Timestamp) -> bool {
        self.not_before.is_none_or(|deadline| deadline <= now)
    }

    pub fn idempotency_key(&self) -> Option<&InvocationId> {
        self.idempotency_key.as_ref()
    }

    pub fn has_idempotency_key(&self, key: &InvocationId) -> bool {
        self.idempotency_key.as_ref() == Some(key)
    }

    pub fn is_manual_update(&self) -> bool {
        self.manual_update_target_revision.is_some()
    }
}

#[cfg(test)]
mod request_equality_tests {
    use super::*;
    use crate::model::deployment::DeploymentRevision;
    use crate::model::entity::FilesystemCapability;
    use crate::model::json::NormalizedJsonValue;
    use crate::model::tool::{
        CompiledToolBinding, ToolBindingOwner, ToolFilesystemAccess, ToolRegistration, ToolSource,
    };
    use crate::schema::tool::{CommandTree, Tool};
    use crate::schema::{SchemaGraph, SchemaType, UnionValuePayload, VariantValuePayload};
    use test_r::test;

    fn tool_request(input: TypedSchemaValue) -> AgentInvocationPayload {
        let source = ToolSource::Component {
            component_id: Default::default(),
            component_revision: ComponentRevision::INITIAL,
            component_name: "test:tool".try_into().unwrap(),
        };
        let tool_name = ToolName::try_from("test").unwrap();
        AgentInvocationPayload::ExternalTool {
            tool_name: tool_name.clone(),
            command_path: vec!["run".to_string()],
            input: Box::new(input),
            stdin: false,
            stdout: false,
            stderr: false,
            activation: Box::new(ToolActivationSnapshot {
                registered_tool: ToolRegistration {
                    deployment_revision: DeploymentRevision::INITIAL,
                    release_id: None,
                    definition: Tool {
                        version: "1.0.0".to_string(),
                        requires_filesystem: false,
                        commands: CommandTree { nodes: Vec::new() },
                        schema: SchemaGraph::empty(),
                    },
                    provision: Default::default(),
                    source: source.clone(),
                    owner_account_id: Default::default(),
                    owner_account_email: "owner@example.com".into(),
                    metadata_version: String::new(),
                    metadata_digest: Default::default(),
                    component_bindings: Default::default(),
                },
                binding: CompiledToolBinding {
                    deployment_revision: DeploymentRevision::INITIAL,
                    release_id: None,
                    owner: ToolBindingOwner::AgentType {
                        agent_type_name: "Test".parse().unwrap(),
                    },
                    tool_name,
                    version: "1.0.0".to_string(),
                    metadata_version: String::new(),
                    metadata_digest: Default::default(),
                    account_id: Default::default(),
                    account_email: "owner@example.com".into(),
                    parameters: NormalizedJsonValue::new(serde_json::Value::Null),
                    config_keys_readable: Default::default(),
                    secret_keys_readable: Default::default(),
                    secret_keys_revealable: Default::default(),
                    filesystem_access: ToolFilesystemAccess::Denied,
                    source,
                },
                filesystem: FilesystemCapability::Incapable,
                middleware_chain: None,
                mcp_import: None,
            }),
            principal: Principal::anonymous(),
            scope_card: None,
        }
    }

    fn requests(value: SchemaValue, schema: SchemaType) -> Vec<AgentInvocationPayload> {
        vec![
            AgentInvocationPayload::AgentInitialization {
                input: value.clone(),
                principal: Principal::anonymous(),
            },
            AgentInvocationPayload::AgentMethod {
                method_name: "run".to_string(),
                input: value.clone(),
                principal: Principal::anonymous(),
                scope_card: None,
            },
            tool_request(TypedSchemaValue::new(SchemaGraph::anonymous(schema), value)),
        ]
    }

    #[test]
    fn same_caller_request_float_bits() {
        for (a, b, schema) in [
            (
                SchemaValue::F32(f32::from_bits(0x7fc0_0001)),
                SchemaValue::F32(f32::from_bits(0x7fc0_0002)),
                SchemaType::f32(),
            ),
            (
                SchemaValue::F64(f64::from_bits(0x7ff8_0000_0000_0001)),
                SchemaValue::F64(f64::from_bits(0x7ff8_0000_0000_0002)),
                SchemaType::f64(),
            ),
            (
                SchemaValue::F32(0.0),
                SchemaValue::F32(-0.0),
                SchemaType::f32(),
            ),
            (
                SchemaValue::F64(0.0),
                SchemaValue::F64(-0.0),
                SchemaType::f64(),
            ),
        ] {
            for (a, b) in requests(a, schema.clone()).iter().zip(requests(b, schema)) {
                assert!(a.same_caller_request(&a.clone()), "{a:?}");
                assert!(!a.same_caller_request(&b), "{a:?} vs {b:?}");
                assert!(!b.same_caller_request(a), "{b:?} vs {a:?}");
                let bytes = desert_rust::serialize_to_byte_vec(a).unwrap();
                let stored: AgentInvocationPayload = desert_rust::deserialize(&bytes).unwrap();
                assert!(stored.same_caller_request(a));
                assert!(!stored.same_caller_request(&b));
            }
        }
    }

    #[test]
    fn same_caller_request_nested_float_bits() {
        fn containers(value: SchemaValue) -> Vec<SchemaValue> {
            vec![
                SchemaValue::Record {
                    fields: vec![value.clone()],
                },
                SchemaValue::Tuple {
                    elements: vec![value.clone()],
                },
                SchemaValue::List {
                    elements: vec![value.clone()],
                },
                SchemaValue::FixedList {
                    elements: vec![value.clone()],
                },
                SchemaValue::Map {
                    entries: vec![(value.clone(), SchemaValue::Bool(true))],
                },
                SchemaValue::Map {
                    entries: vec![(SchemaValue::Bool(true), value.clone())],
                },
                SchemaValue::Variant(VariantValuePayload {
                    case: 1,
                    payload: Some(Box::new(value.clone())),
                }),
                SchemaValue::Option {
                    inner: Some(Box::new(value.clone())),
                },
                SchemaValue::Result(ResultValuePayload::Ok {
                    value: Some(Box::new(value.clone())),
                }),
                SchemaValue::Result(ResultValuePayload::Err {
                    value: Some(Box::new(value.clone())),
                }),
                SchemaValue::Union(UnionValuePayload {
                    tag: "float".to_string(),
                    body: Box::new(value),
                }),
            ]
        }

        for (a, b) in [
            (
                SchemaValue::F32(f32::from_bits(0x7fc0_0001)),
                SchemaValue::F32(f32::from_bits(0x7fc0_0002)),
            ),
            (
                SchemaValue::F64(f64::from_bits(0x7ff8_0000_0000_0001)),
                SchemaValue::F64(f64::from_bits(0x7ff8_0000_0000_0002)),
            ),
            (SchemaValue::F32(0.0), SchemaValue::F32(-0.0)),
            (SchemaValue::F64(0.0), SchemaValue::F64(-0.0)),
        ] {
            for (a, b) in containers(a).into_iter().zip(containers(b)) {
                let a = SchemaValue::Option {
                    inner: Some(Box::new(a)),
                };
                let b = SchemaValue::Option {
                    inner: Some(Box::new(b)),
                };
                assert!(schema_value_same_request(&a, &a.clone()), "{a:?}");
                assert!(!schema_value_same_request(&a, &b), "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn same_caller_request_nested_typed_inputs() {
        let schema = SchemaType::option(SchemaType::list(SchemaType::tuple(vec![
            SchemaType::f32(),
            SchemaType::f64(),
        ])));
        let nested = |a, b| SchemaValue::Option {
            inner: Some(Box::new(SchemaValue::List {
                elements: vec![SchemaValue::Tuple {
                    elements: vec![SchemaValue::F32(a), SchemaValue::F64(b)],
                }],
            })),
        };
        for (a, b, changed_a, changed_b) in [
            (
                f32::from_bits(0x7fc0_0001),
                f64::from_bits(0x7ff8_0000_0000_0001),
                f32::from_bits(0x7fc0_0002),
                f64::from_bits(0x7ff8_0000_0000_0002),
            ),
            (0.0, 0.0, -0.0, -0.0),
        ] {
            let original = requests(nested(a, b), schema.clone());
            for changed in [nested(changed_a, b), nested(a, changed_b)] {
                for (original, changed) in original.iter().zip(requests(changed, schema.clone())) {
                    assert!(original.same_caller_request(&original.clone()));
                    assert!(!original.same_caller_request(&changed));
                }
            }
        }
    }

    #[test]
    fn same_caller_request_tool_schema_is_part_of_identity() {
        let a = tool_request(TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::f32()),
            SchemaValue::F32(0.0),
        ));
        let b = tool_request(TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::f64()),
            SchemaValue::F32(0.0),
        ));
        assert!(!a.same_caller_request(&b));
    }
}
