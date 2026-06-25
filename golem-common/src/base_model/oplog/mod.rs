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

pub mod multipart;
mod oplog_macro;
pub(crate) mod public_types;

use crate::base_model::account::AccountId;
use crate::base_model::agent::AgentMode;
use crate::base_model::component::ComponentRevision;
use crate::base_model::environment::EnvironmentId;
use crate::base_model::invocation_context::SpanId;
use crate::base_model::regions::OplogRegion;
use crate::base_model::{AgentId, IdempotencyKey, OplogIndex, Timestamp, TransactionId};
use crate::oplog_entry;
use crate::schema::TypedSchemaValue;
pub use public_types::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

/// Imports only used by the raw oplog entries - not generated unless the 'full' feature is enabled.
#[cfg(feature = "full")]
mod raw_imports {
    pub use crate::base_model::environment_plugin_grant::EnvironmentPluginGrantId;
    pub use crate::base_model::invocation_context::TraceId;
    pub use crate::model::invocation_context::AttributeValue;
    pub use crate::model::oplog::payload;
    pub use crate::model::oplog::raw_types::AttributeMap;
    pub use crate::model::oplog::raw_types::*;
    pub use crate::model::retry_policy::{NamedRetryPolicy, RetryPolicyState};
    pub use crate::model::worker::UntypedAgentConfigEntry;
    pub use crate::model::{AgentInvocationPayload, AgentInvocationResult};
    pub use crate::resource_runtime::ResourceTypeId;

    pub use std::collections::HashSet;
}

#[cfg(feature = "full")]
use raw_imports::*;

// Generates two primary types:
// - OplogEntry
//
// binary serializable, the actual representation in the oplog.
// the generated enum type contains constructor functions and helpers (timestamp, is_hint, rounded)
//
// - PublicOplogEntry
//
// the oplog representation presented to users through queries, with enriched information
// with JSON and poem codecs and (hand-written) lucene query matching
//
// The macro's DSL requires the following items for each oplog entry to be specified:
// - hint: false|true
// - raw: fields of the OplogEntry case
// - public: fields of the PublicOplogEntry case
oplog_entry! {
    /// The first entry of every oplog
    Create {
        hint: true
        wit_raw_type: "raw-create-parameters"
        wit_public_type: "create-parameters"
        raw {
            agent_id: AgentId,
            agent_mode: AgentMode,
            component_revision: ComponentRevision,
            env: Vec<(String, String)>,
            environment_id: EnvironmentId,
            created_by: AccountId,
            parent: Option<AgentId>,
            component_size: u64,
            initial_total_linear_memory_size: u64,
            initial_active_plugins: HashSet<EnvironmentPluginGrantId>,
            local_agent_config: Vec<UntypedAgentConfigEntry>,
            original_phantom_id: Option<Uuid>,
            /// Per-instance fingerprint, unique across recreations of the same agent ID.
            instance_id: Uuid
        }
        public {
            agent_id: AgentId,
            agent_mode: AgentMode,
            component_revision: ComponentRevision,
            env: BTreeMap<String, String>,
            created_by: AccountId,
            environment_id: EnvironmentId,
            parent: Option<AgentId>,
            component_size: u64,
            initial_total_linear_memory_size: u64,
            initial_active_plugins: BTreeSet<PluginInstallationDescription>,
            local_agent_config: Vec<PublicTypedAgentConfigEntry>,
            original_phantom_id: Option<Uuid>,
            instance_id: Uuid
        }
    },
    /// Marks the start of a durable host call (or scope such as a batched-write).
    ///
    /// A `Start` is identified by its own `OplogIndex`. It is paired either with a
    /// matching `End` (successful completion) or a matching `Cancelled` (the call was
    /// dropped before completion); both reference this `Start` via `start_index`.
    ///
    /// `parent_start_index` is the `OplogIndex` of the enclosing scope's `Start`, if any.
    /// `request` is `Some(...)` for real host calls and `None` for scopes that have no
    /// host-level request payload (batched-write, future transaction scopes).
    Start {
        hint: false
        wit_raw_type: "raw-start-parameters"
        wit_public_type: "start-parameters"
        raw {
            parent_start_index: Option<OplogIndex>,
            function_name: payload::host_functions::HostFunctionName,
            request: Option<payload::OplogPayload<payload::HostRequest>>,
            durable_function_type: DurableFunctionType,
        }
        public {
            parent_start_index: Option<OplogIndex>,
            function_name: String,
            request: Option<TypedSchemaValue>,
            durable_function_type: PublicDurableFunctionType,
        }
    },
    /// Marks the successful completion of a durable host call (or scope) started by the
    /// `Start` at `start_index`.
    ///
    /// `response` is `Some(...)` for real host calls and `None` for scopes (batched-write,
    /// future transaction scopes). `forced_commit` requests the oplog to commit immediately
    /// after this entry is appended (currently only used for scope ends that today drive
    /// `CommitLevel::Always`).
    End {
        hint: false
        wit_raw_type: "raw-end-parameters"
        wit_public_type: "end-parameters"
        raw {
            start_index: OplogIndex,
            response: Option<payload::OplogPayload<payload::HostResponse>>,
            forced_commit: bool,
        }
        public {
            start_index: OplogIndex,
            response: Option<TypedSchemaValue>,
            forced_commit: bool,
        }
    },
    /// Marks that a durable host call started by the `Start` at `start_index` was
    /// cancelled (e.g. dropped from a `select!`) before producing a final response.
    ///
    /// `partial` optionally carries any partial response captured before cancellation
    /// (e.g. partially read bytes from a stream).
    Cancelled {
        hint: false
        wit_raw_type: "raw-cancelled-parameters"
        wit_public_type: "cancelled-parameters"
        raw {
            start_index: OplogIndex,
            partial: Option<payload::OplogPayload<payload::HostResponse>>,
        }
        public {
            start_index: OplogIndex,
            partial: Option<TypedSchemaValue>,
        }
    },
    /// The agent has been invoked
    AgentInvocationStarted {
        hint: false
        wit_raw_type: "raw-agent-invocation-started-parameters"
        wit_public_type: "agent-invocation-started-parameters"
        raw {
            idempotency_key: IdempotencyKey,
            payload: payload::OplogPayload<AgentInvocationPayload>,
            trace_id: TraceId,
            trace_states: Vec<String>,
            invocation_context: Vec<SpanData>,
        }
        public {
            invocation: PublicAgentInvocation,
        }
    },
    /// The agent has completed an invocation
    AgentInvocationFinished {
        hint: false
        wit_raw_type: "raw-agent-invocation-finished-parameters"
        wit_public_type: "agent-invocation-finished-parameters"
        raw {
            result: payload::OplogPayload<AgentInvocationResult>,
            method_name: Option<String>,
            consumed_fuel: i64,
            component_revision: ComponentRevision,
        }
        public {
            result: PublicAgentInvocationResult,
            method_name: Option<String>,
            consumed_fuel: i64,
            component_revision: ComponentRevision,
        }
    },
    /// Worker suspended
    Suspend {
        hint: true
        wit_raw_type: "timestamp"
        wit_public_type: "timestamp"
        raw {}
        public {}
    },
    /// Worker failed
    Error {
        hint: true
        wit_raw_type: "raw-error-parameters"
        wit_public_type: "error-parameters"
        raw {
            error: AgentError,
            /// Points to the oplog index where the retry should start from. Normally this can be just the
            /// current oplog index (after the last persisted side-effect). When failing in an atomic region
            /// or batched remote writes, this should point to the start of the region.
            /// When counting the number of retries for a specific error, the error entries are grouped by this index.
            retry_from: OplogIndex,
            /// Whether the error occurred inside an active atomic region that has already performed side effects.
            /// This affects retry decisions for deterministic traps.
            inside_atomic_region: bool,
            /// Optional semantic retry state. When present, this allows exact reconstruction
            /// of semantic retry policies without count-based replay.
            retry_policy_state: Option<RetryPolicyState>,
        }
        public {
            error: String,
            retry_from: OplogIndex,
            inside_atomic_region: bool,
            retry_policy_state: Option<PublicRetryPolicyState>,
        }
    },
    /// Marker entry added when get-oplog-index is called from the worker, to make the jumping behavior
    /// more predictable.
    NoOp {
        hint: false
        wit_raw_type: "timestamp"
        wit_public_type: "timestamp"
        raw {}
        public {}
    },
    /// The worker needs to recover up to the given target oplog index and continue running from
    /// the source oplog index from there.
    /// `jump` is an oplog region representing that from the end of that region we want to go back to the start and
    /// ignore all recorded operations in between.
    Jump {
        hint: false
        wit_raw_type: "jump-parameters"
        wit_public_type: "jump-parameters"
        raw {
            jump: OplogRegion,
        }
        public {
            jump: OplogRegion,
        }
    },
    /// Indicates that the worker has been interrupted at this point.
    /// Only used to recompute the worker's (cached) status, has no effect on execution.
    Interrupted {
        hint: true
        wit_raw_type: "timestamp"
        wit_public_type: "timestamp"
        raw {}
        public {}
    },
    /// Indicates that the worker has been exited using WASI's exit function.
    Exited {
        hint: true
        wit_raw_type: "timestamp"
        wit_public_type: "timestamp"
        raw {}
        public {}
    },
    /// Begins an atomic region. All oplog entries after `BeginAtomicRegion` are to be ignored during
    /// recovery except if there is a corresponding `EndAtomicRegion` entry.
    BeginAtomicRegion {
        hint: false
        wit_raw_type: "timestamp"
        wit_public_type: "timestamp"
        raw {}
        public {}
    },
    /// Ends an atomic region. All oplog entries between the corresponding `BeginAtomicRegion` and this
    /// entry are to be considered during recovery, and the begin/end markers can be removed during oplog
    /// compaction.
    EndAtomicRegion {
        hint: false
        wit_raw_type: "end-atomic-region-parameters"
        wit_public_type: "end-atomic-region-parameters"
        raw {
            begin_index: OplogIndex,
        }
        public {
            begin_index: OplogIndex,
        }
    },
    /// An invocation request arrived while the worker was busy
    PendingAgentInvocation {
        hint: true
        wit_raw_type: "raw-pending-agent-invocation-parameters"
        wit_public_type: "pending-agent-invocation-parameters"
        raw {
            idempotency_key: IdempotencyKey,
            payload: payload::OplogPayload<AgentInvocationPayload>,
            trace_id: TraceId,
            trace_states: Vec<String>,
            invocation_context: Vec<SpanData>,
        }
        public {
            invocation: PublicAgentInvocation
        }
    },
    /// An update request arrived and will be applied as soon the worker restarts
    ///
    /// For automatic updates worker is expected to immediately get interrupted and restarted after inserting this entry.
    /// For manual updates, this entry is only inserted when the worker is idle, and it is also restarted.
    PendingUpdate {
        hint: true
        wit_raw_type: "raw-pending-update-parameters"
        wit_public_type: "pending-update-parameters"
        raw {
            description: UpdateDescription,
        }
        public {
            target_revision: ComponentRevision,
            description: PublicUpdateDescription,
        }
    },
    /// An update was successfully applied
    SuccessfulUpdate {
        hint: true
        wit_raw_type: "raw-successful-update-parameters"
        wit_public_type: "successful-update-parameters"
        raw {
            target_revision: ComponentRevision,
            new_component_size: u64,
            new_active_plugins: HashSet<EnvironmentPluginGrantId>,
        }
        public {
            target_revision: ComponentRevision,
            new_component_size: u64,
            new_active_plugins: BTreeSet<PluginInstallationDescription>,
        }
    },
    /// An update failed to be applied
    FailedUpdate {
        hint: true
        wit_raw_type: "failed-update-parameters"
        wit_public_type: "failed-update-parameters"
        raw {
            target_revision: ComponentRevision,
            details: Option<String>,
        }
        public {
            target_revision: ComponentRevision,
            details: Option<String>,
        }
    },
    /// Increased total linear memory size
    GrowMemory {
        hint: true
        wit_raw_type: "grow-memory-parameters"
        wit_public_type: "grow-memory-parameters"
        raw {
            delta: u64
        }
        public {
            delta: u64
        }
    },
    /// Updated storage usage by a signed delta (positive = write, negative = delete/shrink)
    FilesystemStorageUsageUpdate {
        hint: true
        wit_raw_type: "filesystem-storage-usage-update-parameters"
        wit_public_type: "filesystem-storage-usage-update-parameters"
        raw {
            delta: i64
        }
        public {
            delta: i64
        }
    },
    /// Created a resource instance
    CreateResource {
        hint: true
        wit_raw_type: "raw-create-resource-parameters"
        wit_public_type: "create-resource-parameters"
        raw {
            id: AgentResourceId,
            resource_type_id: ResourceTypeId,
        }
        public {
            id: AgentResourceId,
            name: String,
            owner: String
        }
    },
    /// Dropped a resource instance
    DropResource {
        hint: true
        wit_raw_type: "raw-drop-resource-parameters"
        wit_public_type: "drop-resource-parameters"
        raw {
            id: AgentResourceId,
            resource_type_id: ResourceTypeId,
        }
        public {
            id: AgentResourceId,
            name: String,
            owner: String
        }
    },
    /// The worker emitted a log message
    Log {
        hint: true
        wit_raw_type: "log-parameters"
        wit_public_type: "log-parameters"
        raw {
            level: LogLevel,
            context: String,
            message: String,
        }
        public {
            level: LogLevel,
            context: String,
            message: String,
        }
    },
    /// Marks the point where the worker was restarted from clean initial state
    Restart {
        hint: true
        wit_raw_type: "timestamp"
        wit_public_type: "timestamp"
        raw {}
        public {}
    },
    /// Activates a plugin for the worker
    ActivatePlugin {
        hint: true
        wit_raw_type: "raw-activate-plugin-parameters"
        wit_public_type: "activate-plugin-parameters"
        raw {
            plugin_grant_id: EnvironmentPluginGrantId,
        }
        public {
            plugin: PluginInstallationDescription
        }
    },
    /// Deactivates a plugin for the worker
    DeactivatePlugin {
        hint: true
        wit_raw_type: "raw-deactivate-plugin-parameters"
        wit_public_type: "deactivate-plugin-parameters"
        raw {
            plugin_grant_id: EnvironmentPluginGrantId,
        }
        public {
            plugin: PluginInstallationDescription
        }
    },
    /// Similar to `Jump` but caused by an external revert request.
    Revert {
        hint: true
        wit_raw_type: "revert-parameters"
        wit_public_type: "revert-parameters"
        raw {
            dropped_region: OplogRegion,
        }
        public {
            dropped_region: OplogRegion,
        }
    },
    /// Removes a pending invocation from the invocation queue
    CancelPendingInvocation {
        hint: true
        wit_raw_type: "cancel-pending-invocation-parameters"
        wit_public_type: "cancel-pending-invocation-parameters"
        raw {
            idempotency_key: IdempotencyKey,
        }
        public {
            idempotency_key: IdempotencyKey,
        }
    },
    /// Starts a new span in the invocation context
    StartSpan {
        hint: false
        wit_raw_type: "start-span-parameters"
        wit_public_type: "start-span-parameters"
        raw {
            span_id: SpanId,
            parent: Option<SpanId>,
            linked_context_id: Option<SpanId>,
            attributes: AttributeMap,
        }
        public {
            span_id: SpanId,
            parent_id: Option<SpanId>,
            linked_context: Option<SpanId>,
            attributes: Vec<PublicAttribute>,
        }
    },
    /// Finishes an open span in the invocation context
    FinishSpan {
        hint: false
        wit_raw_type: "finish-span-parameters"
        wit_public_type: "finish-span-parameters"
        raw {
            span_id: SpanId,
        }
        public {
            span_id: SpanId,
        }
    },
    /// Set an attribute on an open span in the invocation contex
    SetSpanAttribute {
        hint: false
        wit_raw_type: "set-span-attribute-parameters"
        wit_public_type: "set-span-attribute-parameters"
        raw {
            span_id: SpanId,
            key: String,
            value: AttributeValue,
        }
        public {
            span_id: SpanId,
            key: String,
            value: PublicAttributeValue,
        }
    },
    /// Change persistence level
    ChangePersistenceLevel {
        hint: false
        wit_raw_type: "change-persistence-level-parameters"
        wit_public_type: "change-persistence-level-parameters"
        raw {
            persistence_level: PersistenceLevel,
        }
        public {
            persistence_level: PersistenceLevel
        }
    },
    /// Marks the beginning of a remote transaction
    BeginRemoteTransaction {
        hint: false
        wit_raw_type: "raw-begin-remote-transaction-parameters"
        wit_public_type: "begin-remote-transaction-parameters"
        raw {
            transaction_id: TransactionId,
            /// BeginRemoteTransaction entries need to be repeated on retries, because they may need a new
            /// transaction_id. The `begin_index` field always points to the original, first entry. This makes
            /// error grouping work. When None, this is the original begin entry.
            original_begin_index: Option<OplogIndex>,
        }
        public {
            transaction_id: TransactionId
        }
    },
    /// Marks the point before a remote transaction is committed
    PreCommitRemoteTransaction {
        hint: false
        wit_raw_type: "remote-transaction-parameters"
        wit_public_type: "remote-transaction-parameters"
        raw {
            begin_index: OplogIndex,
        }
        public {
            begin_index: OplogIndex,
        }
    },
    /// Marks the point before a remote transaction is rolled back
    PreRollbackRemoteTransaction {
        hint: false
        wit_raw_type: "remote-transaction-parameters"
        wit_public_type: "remote-transaction-parameters"
        raw {
            begin_index: OplogIndex,
        }
        public {
            begin_index: OplogIndex,
        }
    },
    /// Marks the point after a remote transaction is committed
    CommittedRemoteTransaction {
        hint: false
        wit_raw_type: "remote-transaction-parameters"
        wit_public_type: "remote-transaction-parameters"
        raw {
            begin_index: OplogIndex,
        }
        public {
            begin_index: OplogIndex,
        }
    },
    /// Marks the point after a remote transaction is rolled back
    RolledBackRemoteTransaction {
        hint: false
        wit_raw_type: "remote-transaction-parameters"
        wit_public_type: "remote-transaction-parameters"
        raw {
            begin_index: OplogIndex,
        }
        public {
            begin_index: OplogIndex,
        }
    },
    /// A snapshot of the agent's state
    Snapshot {
        hint: true
        wit_raw_type: "raw-snapshot-parameters"
        wit_public_type: "snapshot-parameters"
        raw {
            data: payload::OplogPayload<Vec<u8>>,
            mime_type: String,
        }
        public {
            data: PublicSnapshotData
        }
    },
    /// Checkpoint for oplog processor plugin delivery tracking
    OplogProcessorCheckpoint {
        hint: true
        wit_raw_type: "raw-oplog-processor-checkpoint-parameters"
        wit_public_type: "oplog-processor-checkpoint-parameters"
        raw {
            plugin_grant_id: EnvironmentPluginGrantId,
            target_agent_id: AgentId,
            confirmed_up_to: OplogIndex,
            sending_up_to: OplogIndex,
            last_batch_start: OplogIndex,
        }
        public {
            plugin: PluginInstallationDescription,
            target_agent_id: AgentId,
            confirmed_up_to: OplogIndex,
            sending_up_to: OplogIndex,
            last_batch_start: OplogIndex,
        }
    },
    /// Sets or overwrites a named retry policy (persisted to oplog)
    SetRetryPolicy {
        hint: false
        wit_raw_type: "set-retry-policy-parameters"
        wit_public_type: "set-retry-policy-parameters"
        raw {
            policy: NamedRetryPolicy,
        }
        public {
            policy: PublicNamedRetryPolicy,
        }
    },
    /// Removes a named retry policy by name (persisted to oplog)
    RemoveRetryPolicy {
        hint: false
        wit_raw_type: "remove-retry-policy-parameters"
        wit_public_type: "remove-retry-policy-parameters"
        raw {
            name: String,
        }
        public {
            name: String,
        }
    },
    /// Records that a permission card used by the agent has been revoked.
    CardRevoked {
        hint: true
        wit_raw_type: "card-revoked-parameters"
        wit_public_type: "card-revoked-parameters"
        raw {
            card_id: Uuid,
        }
        public {
            card_id: Uuid,
        }
    }
}
