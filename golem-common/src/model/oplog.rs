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

pub use crate::base_model::OplogIndex;
use crate::model::invocation_context::{AttributeValue, InvocationContextSpan, SpanId, TraceId};
use crate::model::regions::OplogRegion;
use crate::model::{
    AccountId, ComponentVersion, IdempotencyKey, PluginInstallationId, Timestamp, TransactionId,
    WorkerId, WorkerInvocation,
};
use crate::model::{ProjectId, RetryConfig};
use bincode::de::read::Reader;
use bincode::de::{BorrowDecoder, Decoder};
use bincode::enc::write::Writer;
use bincode::enc::Encoder;
use bincode::error::{DecodeError, EncodeError};
use bincode::{BorrowDecode, Decode, Encode};
use golem_wasm::wasmtime::ResourceTypeId;
use golem_wasm_derive::IntoValue;
use nonempty_collections::NEVec;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use uuid::Uuid;

pub struct OplogIndexRange {
    current: u64,
    end: u64,
}

impl Iterator for OplogIndexRange {
    type Item = OplogIndex;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current <= self.end {
            let current = self.current;
            self.current += 1; // Move forward
            Some(OplogIndex(current))
        } else {
            None
        }
    }
}

impl OplogIndexRange {
    pub fn new(start: OplogIndex, end: OplogIndex) -> OplogIndexRange {
        OplogIndexRange {
            current: start.0,
            end: end.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AtomicOplogIndex(Arc<AtomicU64>);

impl AtomicOplogIndex {
    pub fn from_u64(value: u64) -> AtomicOplogIndex {
        AtomicOplogIndex(Arc::new(AtomicU64::new(value)))
    }

    pub fn get(&self) -> OplogIndex {
        OplogIndex(self.0.load(std::sync::atomic::Ordering::Acquire))
    }

    pub fn set(&self, value: OplogIndex) {
        self.0.store(value.0, std::sync::atomic::Ordering::Release);
    }

    pub fn from_oplog_index(value: OplogIndex) -> AtomicOplogIndex {
        AtomicOplogIndex(Arc::new(AtomicU64::new(value.0)))
    }

    /// Gets the previous oplog index
    pub fn previous(&self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }

    /// Gets the next oplog index
    pub fn next(&self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }

    /// Gets the last oplog index belonging to an inclusive range starting at this oplog index,
    /// having `count` elements.
    pub fn range_end(&self, count: u64) {
        self.0
            .fetch_sub(count - 1, std::sync::atomic::Ordering::AcqRel);
    }

    /// Keeps the larger value of this and `other`
    pub fn max(&self, other: OplogIndex) {
        self.0
            .fetch_max(other.0, std::sync::atomic::Ordering::AcqRel);
    }
}

impl Display for AtomicOplogIndex {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.load(std::sync::atomic::Ordering::Acquire))
    }
}

impl From<AtomicOplogIndex> for u64 {
    fn from(value: AtomicOplogIndex) -> Self {
        value.0.load(std::sync::atomic::Ordering::Acquire)
    }
}

impl From<AtomicOplogIndex> for OplogIndex {
    fn from(value: AtomicOplogIndex) -> Self {
        OplogIndex::from_u64(value.0.load(std::sync::atomic::Ordering::Acquire))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayloadId(pub Uuid);

impl Default for PayloadId {
    fn default() -> Self {
        Self::new()
    }
}

impl PayloadId {
    pub fn new() -> PayloadId {
        Self(Uuid::new_v4())
    }
}

impl Display for PayloadId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Encode for PayloadId {
    fn encode<E: Encoder>(&self, encoder: &mut E) -> Result<(), EncodeError> {
        encoder.writer().write(self.0.as_bytes())
    }
}

impl<Context> Decode<Context> for PayloadId {
    fn decode<D: Decoder<Context = Context>>(decoder: &mut D) -> Result<Self, DecodeError> {
        let mut bytes = [0u8; 16];
        decoder.reader().read(&mut bytes)?;
        Ok(Self(Uuid::from_bytes(bytes)))
    }
}

impl<'de, Context> BorrowDecode<'de, Context> for PayloadId {
    fn borrow_decode<D: BorrowDecoder<'de, Context = Context>>(
        decoder: &mut D,
    ) -> Result<Self, DecodeError> {
        let mut bytes = [0u8; 16];
        decoder.reader().read(&mut bytes)?;
        Ok(Self(Uuid::from_bytes(bytes)))
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialOrd,
    Ord,
    PartialEq,
    Eq,
    Hash,
    Encode,
    Decode,
    Serialize,
    Deserialize,
    IntoValue,
    poem_openapi::NewType,
)]
pub struct WorkerResourceId(pub u64);

impl WorkerResourceId {
    pub const INITIAL: WorkerResourceId = WorkerResourceId(0);

    pub fn next(&self) -> WorkerResourceId {
        WorkerResourceId(self.0 + 1)
    }
}

impl Display for WorkerResourceId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Worker log levels including the special stdout and stderr channels
#[derive(
    Copy,
    Clone,
    Debug,
    PartialEq,
    Encode,
    Decode,
    Serialize,
    Deserialize,
    IntoValue,
    poem_openapi::Enum,
)]
#[repr(u8)]
pub enum LogLevel {
    Stdout,
    Stderr,
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Critical,
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum SpanData {
    LocalSpan {
        span_id: SpanId,
        start: Timestamp,
        parent_id: Option<SpanId>,
        linked_context: Option<Vec<SpanData>>,
        attributes: HashMap<String, AttributeValue>,
        inherited: bool,
    },
    ExternalSpan {
        span_id: SpanId,
    },
}

impl SpanData {
    pub fn from_chain(spans: &NEVec<Arc<InvocationContextSpan>>) -> Vec<SpanData> {
        let mut result_spans = Vec::new();
        for span in spans {
            let span_data = match &**span {
                InvocationContextSpan::ExternalParent { span_id } => SpanData::ExternalSpan {
                    span_id: span_id.clone(),
                },
                InvocationContextSpan::Local {
                    span_id,
                    start,
                    state,
                    inherited,
                } => {
                    let state = state.read().unwrap();
                    let parent_id = state.parent.as_ref().map(|parent| parent.span_id().clone());
                    let linked_context = state.linked_context.as_ref().map(|linked| {
                        let linked_chain = linked.to_chain();
                        SpanData::from_chain(&linked_chain)
                    });
                    SpanData::LocalSpan {
                        span_id: span_id.clone(),
                        start: *start,
                        parent_id,
                        linked_context,
                        attributes: state.attributes.clone(),
                        inherited: *inherited,
                    }
                }
            };
            result_spans.push(span_data);
        }
        result_spans
    }
}

#[derive(
    Copy,
    Clone,
    Debug,
    PartialOrd,
    PartialEq,
    Encode,
    Decode,
    Serialize,
    Deserialize,
    IntoValue,
    poem_openapi::Enum,
)]
pub enum PersistenceLevel {
    PersistNothing,
    PersistRemoteSideEffects,
    Smart,
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum OplogEntry {
    /// The worker has completed an invocation
    ExportedFunctionCompleted {
        timestamp: Timestamp,
        response: OplogPayload,
        consumed_fuel: i64,
    },
    /// Worker suspended
    Suspend { timestamp: Timestamp },
    /// Worker failed
    Error {
        timestamp: Timestamp,
        error: WorkerError,
        /// Points to the oplog index where the retry should start from. Normally this can be just the
        /// current oplog index (after the last persisted side-effect). When failing in an atomic region
        /// or batched remote writes, this should point to the start of the region.
        /// When counting the number of retries for a specific error, the error entries are grouped by this index.
        retry_from: OplogIndex,
    },
    /// Marker entry added when get-oplog-index is called from the worker, to make the jumping behavior
    /// more predictable.
    NoOp { timestamp: Timestamp },
    /// The worker needs to recover up to the given target oplog index and continue running from
    /// the source oplog index from there
    /// `jump` is an oplog region representing that from the end of that region we want to go back to the start and
    /// ignore all recorded operations in between.
    Jump {
        timestamp: Timestamp,
        jump: OplogRegion,
    },
    /// Indicates that the worker has been interrupted at this point.
    /// Only used to recompute the worker's (cached) status, has no effect on execution.
    Interrupted { timestamp: Timestamp },
    /// Indicates that the worker has been exited using WASI's exit function.
    Exited { timestamp: Timestamp },
    /// Overrides the worker's retry policy
    ChangeRetryPolicy {
        timestamp: Timestamp,
        new_policy: RetryConfig,
    },
    /// Begins an atomic region. All oplog entries after `BeginAtomicRegion` are to be ignored during
    /// recovery except if there is a corresponding `EndAtomicRegion` entry.
    BeginAtomicRegion { timestamp: Timestamp },
    /// Ends an atomic region. All oplog entries between the corresponding `BeginAtomicRegion` and this
    /// entry are to be considered during recovery, and the begin/end markers can be removed during oplog
    /// compaction.
    EndAtomicRegion {
        timestamp: Timestamp,
        begin_index: OplogIndex,
    },
    /// Begins a remote write operation. Only used when idempotence mode is off. In this case each
    /// remote write must be surrounded by a `BeginRemoteWrite` and `EndRemoteWrite` log pair and
    /// unfinished remote writes cannot be recovered.
    BeginRemoteWrite { timestamp: Timestamp },
    /// Marks the end of a remote write operation. Only used when idempotence mode is off.
    EndRemoteWrite {
        timestamp: Timestamp,
        begin_index: OplogIndex,
    },
    /// An invocation request arrived while the worker was busy
    PendingWorkerInvocation {
        timestamp: Timestamp,
        invocation: WorkerInvocation,
    },
    /// An update request arrived and will be applied as soon the worker restarts
    ///
    /// For automatic updates worker is expected to immediately get interrupted and restarted after inserting this entry.
    /// For manual updates, this entry is only inserted when the worker is idle, and it is also restarted.
    PendingUpdate {
        timestamp: Timestamp,
        description: UpdateDescription,
    },
    /// An update failed to be applied
    FailedUpdate {
        timestamp: Timestamp,
        target_version: ComponentVersion,
        details: Option<String>,
    },
    /// Increased total linear memory size
    GrowMemory { timestamp: Timestamp, delta: u64 },
    /// Created a resource instance
    CreateResource {
        timestamp: Timestamp,
        id: WorkerResourceId,
        resource_type_id: ResourceTypeId,
    },
    /// Dropped a resource instance
    DropResource {
        timestamp: Timestamp,
        id: WorkerResourceId,
        resource_type_id: ResourceTypeId,
    },
    /// The worker emitted a log message
    Log {
        timestamp: Timestamp,
        level: LogLevel,
        context: String,
        message: String,
    },
    /// Marks the point where the worker was restarted from clean initial state
    Restart { timestamp: Timestamp },
    /// The worker invoked a host function
    ImportedFunctionInvoked {
        timestamp: Timestamp,
        function_name: String,
        request: OplogPayload,
        response: OplogPayload,
        durable_function_type: DurableFunctionType,
    },
    /// The first entry of every oplog
    Create {
        timestamp: Timestamp,
        worker_id: WorkerId,
        component_version: ComponentVersion,
        args: Vec<String>,
        env: Vec<(String, String)>,
        project_id: ProjectId,
        created_by: AccountId,
        parent: Option<WorkerId>,
        component_size: u64,
        initial_total_linear_memory_size: u64,
        initial_active_plugins: HashSet<PluginInstallationId>,
        wasi_config_vars: BTreeMap<String, String>,
    },
    /// Activates a plugin for the worker
    ActivatePlugin {
        timestamp: Timestamp,
        plugin: PluginInstallationId,
    },
    /// Deactivates a plugin for the worker
    DeactivatePlugin {
        timestamp: Timestamp,
        plugin: PluginInstallationId,
    },
    /// An update was successfully applied
    SuccessfulUpdate {
        timestamp: Timestamp,
        target_version: ComponentVersion,
        new_component_size: u64,
        new_active_plugins: HashSet<PluginInstallationId>,
    },
    /// Similar to `Jump` but caused by an external revert request. TODO: Golem 2.0 should probably merge with Jump
    Revert {
        timestamp: Timestamp,
        dropped_region: OplogRegion,
    },
    /// Removes a pending invocation from the invocation queue
    CancelPendingInvocation {
        timestamp: Timestamp,
        idempotency_key: IdempotencyKey,
    },
    /// The worker has been invoked
    ExportedFunctionInvoked {
        timestamp: Timestamp,
        function_name: String,
        request: OplogPayload,
        idempotency_key: IdempotencyKey,
        trace_id: TraceId,
        trace_states: Vec<String>,
        invocation_context: Vec<SpanData>,
    },
    /// Starts a new span in the invocation context
    StartSpan {
        timestamp: Timestamp,
        span_id: SpanId,
        parent_id: Option<SpanId>,
        linked_context_id: Option<SpanId>,
        attributes: HashMap<String, AttributeValue>,
    },
    /// Finishes an open span in the invocation context
    FinishSpan {
        timestamp: Timestamp,
        span_id: SpanId,
    },
    /// Set an attribute on an open span in the invocation contex
    SetSpanAttribute {
        timestamp: Timestamp,
        span_id: SpanId,
        key: String,
        value: AttributeValue,
    },
    /// Change persistence level
    ChangePersistenceLevel {
        timestamp: Timestamp,
        level: PersistenceLevel,
    },
    /// Marks the beginning of a remote transaction
    BeginRemoteTransaction {
        timestamp: Timestamp,
        transaction_id: TransactionId,
        /// BeginRemoteTransaction entries need to be repeated on retries, because they may need a new
        /// transaction_id. The `begin_index` field always points to the original, first entry. This makes
        /// error grouping work. When None, this is the original begin entry.
        original_begin_index: Option<OplogIndex>,
    },
    /// Marks the point before a remote transaction is committed
    PreCommitRemoteTransaction {
        timestamp: Timestamp,
        begin_index: OplogIndex,
    },
    /// Marks the point before a remote transaction is rolled back
    PreRollbackRemoteTransaction {
        timestamp: Timestamp,
        begin_index: OplogIndex,
    },
    /// Marks the point after a remote transaction is committed
    CommittedRemoteTransaction {
        timestamp: Timestamp,
        begin_index: OplogIndex,
    },
    /// Marks the point after a remote transaction is rolled back
    RolledBackRemoteTransaction {
        timestamp: Timestamp,
        begin_index: OplogIndex,
    },
}

impl OplogEntry {
    pub fn create(
        worker_id: WorkerId,
        component_version: ComponentVersion,
        args: Vec<String>,
        env: Vec<(String, String)>,
        wasi_config_vars: BTreeMap<String, String>,
        project_id: ProjectId,
        created_by: AccountId,
        parent: Option<WorkerId>,
        component_size: u64,
        initial_total_linear_memory_size: u64,
        initial_active_plugins: HashSet<PluginInstallationId>,
    ) -> OplogEntry {
        OplogEntry::Create {
            timestamp: Timestamp::now_utc(),
            worker_id,
            component_version,
            args,
            env,
            project_id,
            created_by,
            parent,
            component_size,
            initial_total_linear_memory_size,
            initial_active_plugins,
            wasi_config_vars,
        }
    }

    pub fn jump(jump: OplogRegion) -> OplogEntry {
        OplogEntry::Jump {
            timestamp: Timestamp::now_utc(),
            jump,
        }
    }

    pub fn nop() -> OplogEntry {
        OplogEntry::NoOp {
            timestamp: Timestamp::now_utc(),
        }
    }

    pub fn suspend() -> OplogEntry {
        OplogEntry::Suspend {
            timestamp: Timestamp::now_utc(),
        }
    }

    pub fn error(error: WorkerError, retry_from: OplogIndex) -> OplogEntry {
        OplogEntry::Error {
            timestamp: Timestamp::now_utc(),
            error,
            retry_from,
        }
    }

    pub fn interrupted() -> OplogEntry {
        OplogEntry::Interrupted {
            timestamp: Timestamp::now_utc(),
        }
    }

    pub fn exited() -> OplogEntry {
        OplogEntry::Exited {
            timestamp: Timestamp::now_utc(),
        }
    }

    pub fn change_retry_policy(new_policy: RetryConfig) -> OplogEntry {
        OplogEntry::ChangeRetryPolicy {
            timestamp: Timestamp::now_utc(),
            new_policy,
        }
    }

    pub fn begin_atomic_region() -> OplogEntry {
        OplogEntry::BeginAtomicRegion {
            timestamp: Timestamp::now_utc(),
        }
    }

    pub fn end_atomic_region(begin_index: OplogIndex) -> OplogEntry {
        OplogEntry::EndAtomicRegion {
            timestamp: Timestamp::now_utc(),
            begin_index,
        }
    }

    pub fn begin_remote_write() -> OplogEntry {
        OplogEntry::BeginRemoteWrite {
            timestamp: Timestamp::now_utc(),
        }
    }

    pub fn end_remote_write(begin_index: OplogIndex) -> OplogEntry {
        OplogEntry::EndRemoteWrite {
            timestamp: Timestamp::now_utc(),
            begin_index,
        }
    }

    pub fn pending_worker_invocation(invocation: WorkerInvocation) -> OplogEntry {
        OplogEntry::PendingWorkerInvocation {
            timestamp: Timestamp::now_utc(),
            invocation,
        }
    }

    pub fn pending_update(description: UpdateDescription) -> OplogEntry {
        OplogEntry::PendingUpdate {
            timestamp: Timestamp::now_utc(),
            description,
        }
    }

    pub fn successful_update(
        target_version: ComponentVersion,
        new_component_size: u64,
        new_active_plugins: HashSet<PluginInstallationId>,
    ) -> OplogEntry {
        OplogEntry::SuccessfulUpdate {
            timestamp: Timestamp::now_utc(),
            target_version,
            new_component_size,
            new_active_plugins,
        }
    }

    pub fn failed_update(target_version: ComponentVersion, details: Option<String>) -> OplogEntry {
        OplogEntry::FailedUpdate {
            timestamp: Timestamp::now_utc(),
            target_version,
            details,
        }
    }

    pub fn grow_memory(delta: u64) -> OplogEntry {
        OplogEntry::GrowMemory {
            timestamp: Timestamp::now_utc(),
            delta,
        }
    }

    pub fn create_resource(id: WorkerResourceId, resource_type_id: ResourceTypeId) -> OplogEntry {
        OplogEntry::CreateResource {
            timestamp: Timestamp::now_utc(),
            id,
            resource_type_id,
        }
    }

    pub fn drop_resource(id: WorkerResourceId, resource_type_id: ResourceTypeId) -> OplogEntry {
        OplogEntry::DropResource {
            timestamp: Timestamp::now_utc(),
            id,
            resource_type_id,
        }
    }

    pub fn log(level: LogLevel, context: String, message: String) -> OplogEntry {
        OplogEntry::Log {
            timestamp: Timestamp::now_utc(),
            level,
            context,
            message,
        }
    }

    pub fn restart() -> OplogEntry {
        OplogEntry::Restart {
            timestamp: Timestamp::now_utc(),
        }
    }

    pub fn activate_plugin(plugin: PluginInstallationId) -> OplogEntry {
        OplogEntry::ActivatePlugin {
            timestamp: Timestamp::now_utc(),
            plugin,
        }
    }

    pub fn deactivate_plugin(plugin: PluginInstallationId) -> OplogEntry {
        OplogEntry::DeactivatePlugin {
            timestamp: Timestamp::now_utc(),
            plugin,
        }
    }

    pub fn revert(dropped_region: OplogRegion) -> OplogEntry {
        OplogEntry::Revert {
            timestamp: Timestamp::now_utc(),
            dropped_region,
        }
    }

    pub fn cancel_pending_invocation(idempotency_key: IdempotencyKey) -> OplogEntry {
        OplogEntry::CancelPendingInvocation {
            timestamp: Timestamp::now_utc(),
            idempotency_key,
        }
    }

    pub fn start_span(
        timestamp: Timestamp,
        span_id: SpanId,
        parent_id: Option<SpanId>,
        linked_context_id: Option<SpanId>,
        attributes: HashMap<String, AttributeValue>,
    ) -> OplogEntry {
        OplogEntry::StartSpan {
            timestamp,
            span_id,
            parent_id,
            linked_context_id,
            attributes,
        }
    }

    pub fn finish_span(span_id: SpanId) -> OplogEntry {
        OplogEntry::FinishSpan {
            timestamp: Timestamp::now_utc(),
            span_id,
        }
    }

    pub fn set_span_attribute(span_id: SpanId, key: String, value: AttributeValue) -> OplogEntry {
        OplogEntry::SetSpanAttribute {
            timestamp: Timestamp::now_utc(),
            span_id,
            key,
            value,
        }
    }

    pub fn change_persistence_level(level: PersistenceLevel) -> OplogEntry {
        OplogEntry::ChangePersistenceLevel {
            timestamp: Timestamp::now_utc(),
            level,
        }
    }

    pub fn begin_remote_transaction(
        transaction_id: TransactionId,
        original_begin_index: Option<OplogIndex>,
    ) -> OplogEntry {
        OplogEntry::BeginRemoteTransaction {
            timestamp: Timestamp::now_utc(),
            transaction_id,
            original_begin_index,
        }
    }

    pub fn pre_commit_remote_transaction(begin_index: OplogIndex) -> OplogEntry {
        OplogEntry::PreCommitRemoteTransaction {
            timestamp: Timestamp::now_utc(),
            begin_index,
        }
    }

    pub fn pre_rollback_remote_transaction(begin_index: OplogIndex) -> OplogEntry {
        OplogEntry::PreRollbackRemoteTransaction {
            timestamp: Timestamp::now_utc(),
            begin_index,
        }
    }

    pub fn committed_remote_transaction(begin_index: OplogIndex) -> OplogEntry {
        OplogEntry::CommittedRemoteTransaction {
            timestamp: Timestamp::now_utc(),
            begin_index,
        }
    }

    pub fn rolled_back_remote_transaction(begin_index: OplogIndex) -> OplogEntry {
        OplogEntry::RolledBackRemoteTransaction {
            timestamp: Timestamp::now_utc(),
            begin_index,
        }
    }

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

    /// True if the oplog entry is a "hint" that should be skipped during replay
    pub fn is_hint(&self) -> bool {
        matches!(
            self,
            OplogEntry::Suspend { .. }
                | OplogEntry::Error { .. }
                | OplogEntry::Interrupted { .. }
                | OplogEntry::Exited { .. }
                | OplogEntry::PendingWorkerInvocation { .. }
                | OplogEntry::PendingUpdate { .. }
                | OplogEntry::SuccessfulUpdate { .. }
                | OplogEntry::FailedUpdate { .. }
                | OplogEntry::GrowMemory { .. }
                | OplogEntry::CreateResource { .. }
                | OplogEntry::DropResource { .. }
                | OplogEntry::Log { .. }
                | OplogEntry::Restart { .. }
                | OplogEntry::ActivatePlugin { .. }
                | OplogEntry::DeactivatePlugin { .. }
                | OplogEntry::Revert { .. }
                | OplogEntry::CancelPendingInvocation { .. }
        )
    }

    pub fn timestamp(&self) -> Timestamp {
        match self {
            OplogEntry::Create { timestamp, .. }
            | OplogEntry::ExportedFunctionCompleted { timestamp, .. }
            | OplogEntry::Suspend { timestamp }
            | OplogEntry::Error { timestamp, .. }
            | OplogEntry::NoOp { timestamp }
            | OplogEntry::Jump { timestamp, .. }
            | OplogEntry::Interrupted { timestamp }
            | OplogEntry::Exited { timestamp }
            | OplogEntry::ChangeRetryPolicy { timestamp, .. }
            | OplogEntry::BeginAtomicRegion { timestamp }
            | OplogEntry::EndAtomicRegion { timestamp, .. }
            | OplogEntry::BeginRemoteWrite { timestamp }
            | OplogEntry::EndRemoteWrite { timestamp, .. }
            | OplogEntry::PendingWorkerInvocation { timestamp, .. }
            | OplogEntry::PendingUpdate { timestamp, .. }
            | OplogEntry::SuccessfulUpdate { timestamp, .. }
            | OplogEntry::FailedUpdate { timestamp, .. }
            | OplogEntry::GrowMemory { timestamp, .. }
            | OplogEntry::CreateResource { timestamp, .. }
            | OplogEntry::DropResource { timestamp, .. }
            | OplogEntry::Log { timestamp, .. }
            | OplogEntry::Restart { timestamp }
            | OplogEntry::ImportedFunctionInvoked { timestamp, .. }
            | OplogEntry::ActivatePlugin { timestamp, .. }
            | OplogEntry::DeactivatePlugin { timestamp, .. }
            | OplogEntry::Revert { timestamp, .. }
            | OplogEntry::CancelPendingInvocation { timestamp, .. }
            | OplogEntry::ExportedFunctionInvoked { timestamp, .. }
            | OplogEntry::StartSpan { timestamp, .. }
            | OplogEntry::FinishSpan { timestamp, .. }
            | OplogEntry::SetSpanAttribute { timestamp, .. }
            | OplogEntry::ChangePersistenceLevel { timestamp, .. }
            | OplogEntry::BeginRemoteTransaction { timestamp, .. }
            | OplogEntry::PreCommitRemoteTransaction { timestamp, .. }
            | OplogEntry::PreRollbackRemoteTransaction { timestamp, .. }
            | OplogEntry::CommittedRemoteTransaction { timestamp, .. }
            | OplogEntry::RolledBackRemoteTransaction { timestamp, .. } => *timestamp,
        }
    }

    pub fn specifies_component_version(&self) -> Option<ComponentVersion> {
        match self {
            OplogEntry::Create {
                component_version, ..
            } => Some(*component_version),
            OplogEntry::SuccessfulUpdate { target_version, .. } => Some(*target_version),
            _ => None,
        }
    }

    pub fn update_worker_id(&self, worker_id: &WorkerId) -> Option<OplogEntry> {
        match self {
            OplogEntry::Create {
                timestamp,
                component_version,
                args,
                env,
                project_id,
                created_by,
                parent,
                component_size,
                initial_total_linear_memory_size,
                initial_active_plugins,
                wasi_config_vars,
                worker_id: _,
            } => Some(OplogEntry::Create {
                timestamp: *timestamp,
                worker_id: worker_id.clone(),
                component_version: *component_version,
                args: args.clone(),
                env: env.clone(),
                project_id: project_id.clone(),
                created_by: created_by.clone(),
                parent: parent.clone(),
                component_size: *component_size,
                initial_total_linear_memory_size: *initial_total_linear_memory_size,
                initial_active_plugins: initial_active_plugins.clone(),
                wasi_config_vars: wasi_config_vars.clone(),
            }),
            _ => None,
        }
    }
}

/// Describes a pending update
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub enum UpdateDescription {
    /// Automatic update by replaying the oplog on the new version
    Automatic { target_version: ComponentVersion },

    /// Custom update by loading a given snapshot on the new version
    SnapshotBased {
        target_version: ComponentVersion,
        payload: OplogPayload,
    },
}

impl UpdateDescription {
    pub fn target_version(&self) -> &ComponentVersion {
        match self {
            UpdateDescription::Automatic { target_version } => target_version,
            UpdateDescription::SnapshotBased { target_version, .. } => target_version,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct TimestampedUpdateDescription {
    pub timestamp: Timestamp,
    pub oplog_index: OplogIndex,
    pub description: UpdateDescription,
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub enum OplogPayload {
    /// Load the payload from the given byte array
    Inline(Vec<u8>),

    /// Load the payload from the blob storage
    External {
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub enum DurableFunctionType {
    /// The side-effect reads from the worker's local state (for example local file system,
    /// random generator, etc.)
    ReadLocal,
    /// The side-effect writes to the worker's local state (for example local file system)
    WriteLocal,
    /// The side-effect reads from external state (for example a key-value store)
    ReadRemote,
    /// The side-effect manipulates external state (for example an RPC call)
    WriteRemote,
    /// The side-effect manipulates external state through multiple invoked functions (for example
    /// a HTTP request where reading the response involves multiple host function calls)
    ///
    /// On the first invocation of the batch, the parameter should be `None` - this triggers
    /// writing a `BeginRemoteWrite` entry in the oplog. Followup invocations should contain
    /// this entry's index as the parameter. In batched remote writes it is the caller's responsibility
    /// to manually write an `EndRemoteWrite` entry (using `end_function`) when the operation is completed.
    WriteRemoteBatched(Option<OplogIndex>),

    WriteRemoteTransaction(Option<OplogIndex>),
}

/// Describes the error that occurred in the worker
#[derive(Clone, Debug, PartialEq, Eq, Hash, Encode, Decode)]
pub enum WorkerError {
    Unknown(String),
    InvalidRequest(String),
    StackOverflow,
    OutOfMemory,
    // The worker tried to grow its memory beyond the limits of the plan
    ExceededMemoryLimit,
}

impl WorkerError {
    pub fn message(&self) -> &str {
        match self {
            Self::Unknown(message) => message,
            Self::InvalidRequest(message) => message,
            Self::StackOverflow => "Stack overflow",
            Self::OutOfMemory => "Out of memory",
            Self::ExceededMemoryLimit => "Exceeded plan memory limit",
        }
    }

    pub fn to_string(&self, error_logs: &str) -> String {
        let message = self.message();
        let error_logs = if !error_logs.is_empty() {
            format!("\n\n{error_logs}")
        } else {
            "".to_string()
        };
        format!("{message}{error_logs}")
    }
}

mod protobuf {
    use super::WorkerError;
    use crate::model::oplog::PersistenceLevel;

    impl From<PersistenceLevel> for golem_api_grpc::proto::golem::worker::PersistenceLevel {
        fn from(value: PersistenceLevel) -> Self {
            match value {
                PersistenceLevel::PersistNothing => {
                    golem_api_grpc::proto::golem::worker::PersistenceLevel::PersistNothing
                }
                PersistenceLevel::PersistRemoteSideEffects => {
                    golem_api_grpc::proto::golem::worker::PersistenceLevel::PersistRemoteSideEffects
                }
                PersistenceLevel::Smart => {
                    golem_api_grpc::proto::golem::worker::PersistenceLevel::Smart
                }
            }
        }
    }

    impl From<golem_api_grpc::proto::golem::worker::PersistenceLevel> for PersistenceLevel {
        fn from(value: golem_api_grpc::proto::golem::worker::PersistenceLevel) -> Self {
            match value {
                golem_api_grpc::proto::golem::worker::PersistenceLevel::PersistNothing => PersistenceLevel::PersistNothing,
                golem_api_grpc::proto::golem::worker::PersistenceLevel::PersistRemoteSideEffects => PersistenceLevel::PersistRemoteSideEffects,
                golem_api_grpc::proto::golem::worker::PersistenceLevel::Smart => PersistenceLevel::Smart,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::worker::WorkerError> for WorkerError {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::worker::WorkerError,
        ) -> Result<Self, Self::Error> {
            use golem_api_grpc::proto::golem::worker::worker_error::Error;
            match value.error.ok_or("no error field")? {
                Error::StackOverflow(_) => Ok(Self::StackOverflow),
                Error::OutOfMemory(_) => Ok(Self::OutOfMemory),
                Error::InvalidRequest(inner) => Ok(Self::InvalidRequest(inner.details)),
                Error::UnknownError(inner) => Ok(Self::Unknown(inner.details)),
                Error::ExceededMemoryLimit(_) => Ok(Self::ExceededMemoryLimit),
            }
        }
    }

    impl From<WorkerError> for golem_api_grpc::proto::golem::worker::WorkerError {
        fn from(value: WorkerError) -> Self {
            use golem_api_grpc::proto::golem::worker as grpc_worker;
            use golem_api_grpc::proto::golem::worker::worker_error::Error;
            let error = match value {
                WorkerError::StackOverflow => Error::StackOverflow(grpc_worker::StackOverflow {}),
                WorkerError::OutOfMemory => Error::OutOfMemory(grpc_worker::OutOfMemory {}),
                WorkerError::InvalidRequest(details) => {
                    Error::InvalidRequest(grpc_worker::InvalidRequest { details })
                }
                WorkerError::Unknown(details) => {
                    Error::UnknownError(grpc_worker::UnknownError { details })
                }
                WorkerError::ExceededMemoryLimit => {
                    Error::ExceededMemoryLimit(grpc_worker::ExceededMemoryLimit {})
                }
            };
            Self { error: Some(error) }
        }
    }
}
