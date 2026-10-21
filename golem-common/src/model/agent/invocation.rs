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
use crate::model::{AgentFingerprint, AgentId, IdempotencyKey, Timestamp};
use crate::schema::{ResultValuePayload, SchemaValue, TypedSchemaValue};

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
        idempotency_key: IdempotencyKey,
        input: SchemaValue,
        invocation_context: InvocationContextStack,
        principal: Principal,
    },
    AgentMethod {
        idempotency_key: IdempotencyKey,
        method_name: String,
        input: SchemaValue,
        invocation_context: InvocationContextStack,
        principal: Principal,
        scope_card: Option<ScopeCard>,
    },
    ExternalTool {
        idempotency_key: IdempotencyKey,
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
        idempotency_key: IdempotencyKey,
        snapshot: RawSnapshotData, // TODO
    },
    SaveSnapshot {
        idempotency_key: IdempotencyKey,
        // TODO
    },
    ProcessOplogEntries {
        idempotency_key: IdempotencyKey,
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
    pub idempotency_key: Option<IdempotencyKey>,
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
        idempotency_key: IdempotencyKey,
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

    pub fn into_parts(
        self,
    ) -> (
        IdempotencyKey,
        AgentInvocationPayload,
        InvocationContextStack,
    ) {
        match self {
            Self::ManualUpdate { target_revision } => (
                IdempotencyKey::fresh(),
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

    pub fn has_idempotency_key(&self, key: &IdempotencyKey) -> bool {
        self.idempotency_key() == Some(key)
    }

    pub fn idempotency_key(&self) -> Option<&IdempotencyKey> {
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
    /// Index of the `PendingAgentInvocation` oplog entry holding the full payload.
    pub oplog_index: OplogIndex,
    /// Semantic idempotency key of the invocation. `None` for manual updates.
    pub idempotency_key: Option<IdempotencyKey>,
    /// Target revision of the manual update. `Some` only for manual update invocations.
    pub manual_update_target_revision: Option<ComponentRevision>,
}

impl PendingInvocationRef {
    pub fn idempotency_key(&self) -> Option<&IdempotencyKey> {
        self.idempotency_key.as_ref()
    }

    pub fn has_idempotency_key(&self, key: &IdempotencyKey) -> bool {
        self.idempotency_key.as_ref() == Some(key)
    }

    pub fn is_manual_update(&self) -> bool {
        self.manual_update_target_revision.is_some()
    }
}
