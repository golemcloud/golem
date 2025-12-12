// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

pub mod matcher;
mod oplog_macro;
pub mod payload;
mod protobuf;
mod public_types;
mod raw_types;

#[cfg(test)]
mod tests;

pub use crate::base_model::OplogIndex;
pub use payload::*;
pub use public_types::*;
pub use raw_types::*;

use crate::model::component::{ComponentRevision, PluginPriority};
use crate::model::environment::EnvironmentId;
use crate::model::invocation_context::{AttributeValue, SpanId, TraceId};
use crate::model::oplog::host_functions::HostFunctionName;
use crate::model::regions::OplogRegion;
use crate::model::worker::WasiConfigVars;
use crate::model::RetryConfig;
use crate::model::{
    AccountId, IdempotencyKey, Timestamp, TransactionId, WorkerId, WorkerInvocation,
};
use crate::{declare_structs, oplog_entry};
use desert_rust::BinaryCodec;
use golem_wasm::wasmtime::ResourceTypeId;
use golem_wasm::{Value, ValueAndType};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use uuid::Uuid;

// Generates two primary types:
// - OplogEntry
//
// binary serializable, the actual representation in the oplog.
// the generated enum type contains constructor functions and helpers (timestamp, is_hint, rounded)
//
// - PublicOplogEntry
//
// the oplog representation presented to users through queries, with enriched information
// with JSON and poem codecs, convertible to/from golem_wasm::Value and (hand-written) lucene query matching
//
// The macro's DSL requires the following items for each oplog entry to be specified:
// - hint: false|true
// - raw: fields of the OplogEntry case
// - public: fields of the PublicOplogEntry case
oplog_entry! {
    /// The first entry of every oplog
    Create {
        hint: false
        raw {
            worker_id: WorkerId,
            component_revision: ComponentRevision,
            env: Vec<(String, String)>,
            environment_id: EnvironmentId,
            created_by: AccountId,
            parent: Option<WorkerId>,
            component_size: u64,
            initial_total_linear_memory_size: u64,
            initial_active_plugins: HashSet<PluginPriority>,
            wasi_config_vars: BTreeMap<String, String>,
            original_phantom_id: Option<Uuid>
        }
        public {
            worker_id: WorkerId,
            component_revision: ComponentRevision,
            env: BTreeMap<String, String>,
            created_by: AccountId,
            environment_id: EnvironmentId,
            parent: Option<WorkerId>,
            component_size: u64,
            initial_total_linear_memory_size: u64,
            initial_active_plugins: BTreeSet<PluginInstallationDescription>,
            wasi_config_vars: WasiConfigVars,
            original_phantom_id: Option<Uuid>
        }
    },
    /// The worker invoked a host function
    ImportedFunctionInvoked {
        hint: false
        raw {
            function_name: HostFunctionName,
            request: OplogPayload<HostRequest>,
            response: OplogPayload<HostResponse>,
            durable_function_type: DurableFunctionType,
        }
        public {
            function_name: String,
            request: ValueAndType,
            response: ValueAndType,
            durable_function_type: PublicDurableFunctionType,
        }
    },
    /// The worker has been invoked
    ExportedFunctionInvoked {
        hint: false
        raw {
            function_name: String,
            request: OplogPayload<Vec<Value>>,
            idempotency_key: IdempotencyKey,
            trace_id: TraceId,
            trace_states: Vec<String>,
            invocation_context: Vec<SpanData>,
        }
        public {
            function_name: String,
            request: Vec<ValueAndType>,
            idempotency_key: IdempotencyKey,
            trace_id: TraceId,
            trace_states: Vec<String>,
            invocation_context: Vec<Vec<PublicSpanData>>,
        }
    },
    /// The worker has completed an invocation
    ExportedFunctionCompleted {
        hint: false
        raw {
            response: OplogPayload<Option<ValueAndType>>,
            consumed_fuel: i64,
        }
        public {
            response: Option<ValueAndType>,
            consumed_fuel: i64,
        }
    },
    /// Worker suspended
    Suspend {
        hint: true
        raw {}
        public {}
    },
    /// Worker failed
    Error {
        hint: true
        raw {
            error: WorkerError,
            /// Points to the oplog index where the retry should start from. Normally this can be just the
            /// current oplog index (after the last persisted side-effect). When failing in an atomic region
            /// or batched remote writes, this should point to the start of the region.
            /// When counting the number of retries for a specific error, the error entries are grouped by this index.
            retry_from: OplogIndex,
        }
        public {
            error: String,
            retry_from: OplogIndex,
        }
    },
    /// Marker entry added when get-oplog-index is called from the worker, to make the jumping behavior
    /// more predictable.
    NoOp {
        hint: false
        raw {}
        public {}
    },
    /// The worker needs to recover up to the given target oplog index and continue running from
    /// the source oplog index from there.
    /// `jump` is an oplog region representing that from the end of that region we want to go back to the start and
    /// ignore all recorded operations in between.
    Jump {
        hint: false
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
        raw {}
        public {}
    },
    /// Indicates that the worker has been exited using WASI's exit function.
    Exited {
        hint: true
        raw {}
        public {}
    },
    /// Overrides the worker's retry policy
    ChangeRetryPolicy {
        hint: false
        raw {
            new_policy: RetryConfig,
        }
        public {
            new_policy: PublicRetryConfig,
        }
    },
    /// Begins an atomic region. All oplog entries after `BeginAtomicRegion` are to be ignored during
    /// recovery except if there is a corresponding `EndAtomicRegion` entry.
    BeginAtomicRegion {
        hint: false
        raw {}
        public {}
    },
    /// Ends an atomic region. All oplog entries between the corresponding `BeginAtomicRegion` and this
    /// entry are to be considered during recovery, and the begin/end markers can be removed during oplog
    /// compaction.
    EndAtomicRegion {
        hint: false
        raw {
            begin_index: OplogIndex,
        }
        public {
            begin_index: OplogIndex,
        }
    },
    /// Begins a remote write operation. Only used when idempotence mode is off. In this case each
    /// remote write must be surrounded by a `BeginRemoteWrite` and `EndRemoteWrite` log pair and
    /// unfinished remote writes cannot be recovered.
    BeginRemoteWrite {
        hint: false
        raw {}
        public {}
    },
    /// Marks the end of a remote write operation. Only used when idempotence mode is off.
    EndRemoteWrite {
        hint: false
        raw {
            begin_index: OplogIndex,
        }
        public {
            begin_index: OplogIndex,
        }
    },
    /// An invocation request arrived while the worker was busy
    PendingWorkerInvocation {
        hint: true
        raw {
            invocation: WorkerInvocation,
        }
        public {
            invocation: PublicWorkerInvocation
        }
    },
    /// An update request arrived and will be applied as soon the worker restarts
    ///
    /// For automatic updates worker is expected to immediately get interrupted and restarted after inserting this entry.
    /// For manual updates, this entry is only inserted when the worker is idle, and it is also restarted.
    PendingUpdate {
        hint: true
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
        raw {
            target_revision: ComponentRevision,
            new_component_size: u64,
            new_active_plugins: HashSet<PluginPriority>,
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
        raw {
            delta: u64
        }
        public {
            delta: u64
        }
    },
    /// Created a resource instance
    CreateResource {
        hint: true
        raw {
            id: WorkerResourceId,
            resource_type_id: ResourceTypeId,
        }
        public {
            id: WorkerResourceId,
            name: String,
            owner: String
        }
    },
    /// Dropped a resource instance
    DropResource {
        hint: true
        raw {
            id: WorkerResourceId,
            resource_type_id: ResourceTypeId,
        }
        public {
            id: WorkerResourceId,
            name: String,
            owner: String
        }
    },
    /// The worker emitted a log message
    Log {
        hint: true
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
        raw {}
        public {}
    },
    /// Activates a plugin for the worker
    ActivatePlugin {
        hint: true
        raw {
            plugin_priority: PluginPriority,
        }
        public {
            plugin: PluginInstallationDescription
        }
    },
    /// Deactivates a plugin for the worker
    DeactivatePlugin {
        hint: true
        raw {
            plugin_priority: PluginPriority,
        }
        public {
            plugin: PluginInstallationDescription
        }
    },
    /// Similar to `Jump` but caused by an external revert request.
    Revert {
        hint: true
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
        raw {
            span_id: SpanId,
            parent_id: Option<SpanId>,
            linked_context_id: Option<SpanId>,
            attributes: HashMap<String, AttributeValue>,
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
        raw {
            level: PersistenceLevel,
        }
        public {
            persistence_level: PersistenceLevel
        }
    },
    /// Marks the beginning of a remote transaction
    BeginRemoteTransaction {
        hint: false
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
        raw {
            begin_index: OplogIndex,
        }
        public {
            begin_index: OplogIndex,
        }
    },
}

impl OplogEntry {
    pub fn is_end_atomic_region(&self, idx: OplogIndex) -> bool {
        matches!(self, OplogEntry::EndAtomicRegion { begin_index, .. } if *begin_index == idx)
    }

    pub fn is_end_remote_write(&self, idx: OplogIndex) -> bool {
        matches!(self, OplogEntry::EndRemoteWrite { begin_index, .. } if *begin_index == idx)
    }

    pub fn is_end_remote_write_s<S>(&self, idx: OplogIndex, _: &S) -> bool {
        matches!(self, OplogEntry::EndRemoteWrite { begin_index, .. } if *begin_index == idx)
    }

    pub fn is_pre_commit_remote_transaction(&self, idx: OplogIndex) -> bool {
        matches!(self, OplogEntry::PreCommitRemoteTransaction { begin_index, .. } if *begin_index == idx)
    }

    pub fn is_pre_rollback_remote_transaction(&self, idx: OplogIndex) -> bool {
        matches!(self, OplogEntry::PreRollbackRemoteTransaction { begin_index, .. } if *begin_index == idx)
    }

    pub fn is_pre_remote_transaction(&self, idx: OplogIndex) -> bool {
        self.is_pre_commit_remote_transaction(idx) || self.is_pre_rollback_remote_transaction(idx)
    }

    pub fn is_pre_remote_transaction_s<S>(&self, idx: OplogIndex, _: &S) -> bool {
        self.is_pre_commit_remote_transaction(idx) || self.is_pre_rollback_remote_transaction(idx)
    }

    pub fn is_committed_remote_transaction(&self, idx: OplogIndex) -> bool {
        matches!(self, OplogEntry::CommittedRemoteTransaction { begin_index, .. } if *begin_index == idx)
    }

    pub fn is_committed_remote_transaction_s<S>(&self, idx: OplogIndex, _: &S) -> bool {
        matches!(self, OplogEntry::CommittedRemoteTransaction { begin_index, .. } if *begin_index == idx)
    }

    pub fn is_rolled_back_remote_transaction(&self, idx: OplogIndex) -> bool {
        matches!(self, OplogEntry::RolledBackRemoteTransaction { begin_index, .. } if *begin_index == idx)
    }

    pub fn is_rolled_back_remote_transaction_s<S>(&self, idx: OplogIndex, _: &S) -> bool {
        matches!(self, OplogEntry::RolledBackRemoteTransaction { begin_index, .. } if *begin_index == idx)
    }

    pub fn is_end_remote_transaction(&self, idx: OplogIndex) -> bool {
        self.is_committed_remote_transaction(idx) || self.is_rolled_back_remote_transaction(idx)
    }

    pub fn is_end_remote_transaction_s<S>(&self, idx: OplogIndex, s: &S) -> bool {
        self.is_committed_remote_transaction_s(idx, s)
            || self.is_rolled_back_remote_transaction_s(idx, s)
    }

    /// Checks that an "intermediate oplog entry" between a `BeginRemoteWrite` and an `EndRemoteWrite`
    /// is not a RemoteWrite entry which does not belong to the batched remote write started at `idx`.
    /// Side effects in a PersistenceLevel::PersistNothing region are ignored.
    pub fn no_concurrent_side_effect(
        &self,
        idx: OplogIndex,
        persistence_level: &PersistenceLevel,
    ) -> bool {
        if persistence_level == &PersistenceLevel::PersistNothing {
            true
        } else {
            match self {
                OplogEntry::ImportedFunctionInvoked {
                    durable_function_type,
                    ..
                } => match durable_function_type {
                    DurableFunctionType::WriteRemoteBatched(Some(begin_index))
                        if *begin_index == idx =>
                    {
                        true
                    }
                    DurableFunctionType::WriteRemoteTransaction(Some(begin_index))
                        if *begin_index == idx =>
                    {
                        true
                    }
                    DurableFunctionType::ReadLocal => true,
                    DurableFunctionType::WriteLocal => true,
                    DurableFunctionType::ReadRemote => true,
                    _ => false,
                },
                OplogEntry::ExportedFunctionCompleted { .. } => false,
                _ => true,
            }
        }
    }

    pub fn track_persistence_level(
        &self,
        _idx: OplogIndex,
        persistence_level: &mut PersistenceLevel,
    ) {
        if let OplogEntry::ChangePersistenceLevel { level, .. } = self {
            *persistence_level = *level
        }
    }

    pub fn specifies_component_revision(&self) -> Option<ComponentRevision> {
        match self {
            OplogEntry::Create {
                component_revision, ..
            } => Some(*component_revision),
            OplogEntry::SuccessfulUpdate {
                target_revision, ..
            } => Some(*target_revision),
            _ => None,
        }
    }
}

declare_structs! {
    pub struct PublicOplogEntryWithIndex {
        pub oplog_index: OplogIndex,
        pub entry: PublicOplogEntry,
    }
}
