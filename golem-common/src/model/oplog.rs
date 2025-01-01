// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::model::regions::OplogRegion;
use crate::model::RetryConfig;
use crate::model::{
    AccountId, ComponentVersion, IdempotencyKey, PluginInstallationId, Timestamp, WorkerId,
    WorkerInvocation,
};
use bincode::de::read::Reader;
use bincode::de::{BorrowDecoder, Decoder};
use bincode::enc::write::Writer;
use bincode::enc::Encoder;
use bincode::error::{DecodeError, EncodeError};
use bincode::{BorrowDecode, Decode, Encode};
use golem_wasm_ast::analysis::analysed_type::{r#enum, u64};
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::{IntoValue, Value};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt::{Display, Formatter};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use uuid::Uuid;

#[derive(
    Debug,
    Copy,
    Clone,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    Encode,
    Decode,
    Default,
)]
#[cfg_attr(feature = "poem", derive(poem_openapi::NewType))]
pub struct OplogIndex(u64);

impl OplogIndex {
    pub const NONE: OplogIndex = OplogIndex(0);
    pub const INITIAL: OplogIndex = OplogIndex(1);

    pub const fn from_u64(value: u64) -> OplogIndex {
        OplogIndex(value)
    }

    /// Gets the previous oplog index
    pub fn previous(&self) -> OplogIndex {
        OplogIndex(self.0 - 1)
    }

    /// Gets the next oplog index
    pub fn next(&self) -> OplogIndex {
        OplogIndex(self.0 + 1)
    }

    /// Gets the last oplog index belonging to an inclusive range starting at this oplog index,
    /// having `count` elements.
    pub fn range_end(&self, count: u64) -> OplogIndex {
        OplogIndex(self.0 + count - 1)
    }
}

impl Display for OplogIndex {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<OplogIndex> for u64 {
    fn from(value: OplogIndex) -> Self {
        value.0
    }
}

impl IntoValue for OplogIndex {
    fn into_value(self) -> Value {
        Value::U64(self.0)
    }

    fn get_type() -> AnalysedType {
        u64()
    }
}

#[derive(Clone)]
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

impl Decode for PayloadId {
    fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
        let mut bytes = [0u8; 16];
        decoder.reader().read(&mut bytes)?;
        Ok(Self(Uuid::from_bytes(bytes)))
    }
}

impl<'de> BorrowDecode<'de> for PayloadId {
    fn borrow_decode<D: BorrowDecoder<'de>>(decoder: &mut D) -> Result<Self, DecodeError> {
        let mut bytes = [0u8; 16];
        decoder.reader().read(&mut bytes)?;
        Ok(Self(Uuid::from_bytes(bytes)))
    }
}

#[derive(
    Debug, Clone, Copy, PartialOrd, Ord, PartialEq, Eq, Hash, Encode, Decode, Serialize, Deserialize,
)]
#[cfg_attr(feature = "poem", derive(poem_openapi::NewType))]
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

impl IntoValue for WorkerResourceId {
    fn into_value(self) -> Value {
        Value::U64(self.0)
    }

    fn get_type() -> AnalysedType {
        u64()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub struct IndexedResourceKey {
    pub resource_name: String,
    pub resource_params: Vec<String>,
}

/// Worker log levels including the special stdout and stderr channels
#[derive(Copy, Clone, Debug, PartialEq, Encode, Decode, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Enum))]
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

impl IntoValue for LogLevel {
    fn into_value(self) -> Value {
        match self {
            LogLevel::Stdout => Value::Enum(0),
            LogLevel::Stderr => Value::Enum(1),
            LogLevel::Trace => Value::Enum(2),
            LogLevel::Debug => Value::Enum(3),
            LogLevel::Info => Value::Enum(4),
            LogLevel::Warn => Value::Enum(5),
            LogLevel::Error => Value::Enum(6),
            LogLevel::Critical => Value::Enum(7),
        }
    }

    fn get_type() -> AnalysedType {
        r#enum(&[
            "stdout", "stderr", "trace", "debug", "info", "warn", "error", "critical",
        ])
    }
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum OplogEntry {
    CreateV1 {
        timestamp: Timestamp,
        worker_id: WorkerId,
        component_version: ComponentVersion,
        args: Vec<String>,
        env: Vec<(String, String)>,
        account_id: AccountId,
        parent: Option<WorkerId>,
        component_size: u64,
        initial_total_linear_memory_size: u64,
    },
    /// The worker invoked a host function (original 1.0 version)
    ImportedFunctionInvokedV1 {
        timestamp: Timestamp,
        function_name: String,
        response: OplogPayload,
        wrapped_function_type: WrappedFunctionType,
    },
    /// The worker has been invoked
    ExportedFunctionInvoked {
        timestamp: Timestamp,
        function_name: String,
        request: OplogPayload,
        idempotency_key: IdempotencyKey,
    },
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
    PendingUpdate {
        timestamp: Timestamp,
        description: UpdateDescription,
    },
    /// An update was successfully applied
    SuccessfulUpdateV1 {
        timestamp: Timestamp,
        target_version: ComponentVersion,
        new_component_size: u64,
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
    },
    /// Dropped a resource instance
    DropResource {
        timestamp: Timestamp,
        id: WorkerResourceId,
    },
    /// Adds additional information for a created resource instance
    DescribeResource {
        timestamp: Timestamp,
        id: WorkerResourceId,
        indexed_resource: IndexedResourceKey,
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
        wrapped_function_type: WrappedFunctionType,
    },
    /// The current version of the Create entry (previous is CreateV1)
    Create {
        timestamp: Timestamp,
        worker_id: WorkerId,
        component_version: ComponentVersion,
        args: Vec<String>,
        env: Vec<(String, String)>,
        account_id: AccountId,
        parent: Option<WorkerId>,
        component_size: u64,
        initial_total_linear_memory_size: u64,
        initial_active_plugins: HashSet<PluginInstallationId>,
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
}

impl OplogEntry {
    pub fn create(
        worker_id: WorkerId,
        component_version: ComponentVersion,
        args: Vec<String>,
        env: Vec<(String, String)>,
        account_id: AccountId,
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
            account_id,
            parent,
            component_size,
            initial_total_linear_memory_size,
            initial_active_plugins,
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

    pub fn error(error: WorkerError) -> OplogEntry {
        OplogEntry::Error {
            timestamp: Timestamp::now_utc(),
            error,
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

    pub fn create_resource(id: WorkerResourceId) -> OplogEntry {
        OplogEntry::CreateResource {
            timestamp: Timestamp::now_utc(),
            id,
        }
    }

    pub fn drop_resource(id: WorkerResourceId) -> OplogEntry {
        OplogEntry::DropResource {
            timestamp: Timestamp::now_utc(),
            id,
        }
    }

    pub fn describe_resource(
        id: WorkerResourceId,
        indexed_resource: IndexedResourceKey,
    ) -> OplogEntry {
        OplogEntry::DescribeResource {
            timestamp: Timestamp::now_utc(),
            id,
            indexed_resource,
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

    pub fn is_end_atomic_region(&self, idx: OplogIndex) -> bool {
        matches!(self, OplogEntry::EndAtomicRegion { begin_index, .. } if *begin_index == idx)
    }

    pub fn is_end_remote_write(&self, idx: OplogIndex) -> bool {
        matches!(self, OplogEntry::EndRemoteWrite { begin_index, .. } if *begin_index == idx)
    }

    /// Checks that an "intermediate oplog entry" between a `BeginRemoteWrite` and an `EndRemoteWrite`
    /// is not a RemoteWrite entry which does not belong to the batched remote write started at `idx`.
    pub fn no_concurrent_side_effect(&self, idx: OplogIndex) -> bool {
        match self {
            OplogEntry::ImportedFunctionInvoked {
                wrapped_function_type,
                ..
            } => match wrapped_function_type {
                WrappedFunctionType::WriteRemoteBatched(Some(begin_index))
                    if *begin_index == idx =>
                {
                    true
                }
                WrappedFunctionType::ReadLocal => true,
                WrappedFunctionType::WriteLocal => true,
                WrappedFunctionType::ReadRemote => true,
                _ => false,
            },
            OplogEntry::ExportedFunctionCompleted { .. } => false,
            _ => true,
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
                | OplogEntry::SuccessfulUpdateV1 { .. }
                | OplogEntry::FailedUpdate { .. }
                | OplogEntry::GrowMemory { .. }
                | OplogEntry::CreateResource { .. }
                | OplogEntry::DropResource { .. }
                | OplogEntry::DescribeResource { .. }
                | OplogEntry::Log { .. }
                | OplogEntry::Restart { .. }
                | OplogEntry::ActivatePlugin { .. }
                | OplogEntry::DeactivatePlugin { .. }
        )
    }

    pub fn timestamp(&self) -> Timestamp {
        match self {
            OplogEntry::Create { timestamp, .. }
            | OplogEntry::ImportedFunctionInvokedV1 { timestamp, .. }
            | OplogEntry::ExportedFunctionInvoked { timestamp, .. }
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
            | OplogEntry::DescribeResource { timestamp, .. }
            | OplogEntry::Log { timestamp, .. }
            | OplogEntry::Restart { timestamp }
            | OplogEntry::ImportedFunctionInvoked { timestamp, .. }
            | OplogEntry::CreateV1 { timestamp, .. }
            | OplogEntry::SuccessfulUpdateV1 { timestamp, .. }
            | OplogEntry::ActivatePlugin { timestamp, .. }
            | OplogEntry::DeactivatePlugin { timestamp, .. } => *timestamp,
        }
    }

    pub fn specifies_component_version(&self) -> Option<ComponentVersion> {
        match self {
            OplogEntry::Create {
                component_version, ..
            } => Some(*component_version),
            OplogEntry::CreateV1 {
                component_version, ..
            } => Some(*component_version),
            OplogEntry::SuccessfulUpdate { target_version, .. } => Some(*target_version),
            OplogEntry::SuccessfulUpdateV1 { target_version, .. } => Some(*target_version),
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
pub enum WrappedFunctionType {
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
}

/// Describes the error that occurred in the worker
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub enum WorkerError {
    Unknown(String),
    InvalidRequest(String),
    StackOverflow,
    OutOfMemory,
}

impl WorkerError {
    pub fn to_string(&self, error_logs: &str) -> String {
        let error_logs = if !error_logs.is_empty() {
            format!("\n\n{}", error_logs)
        } else {
            "".to_string()
        };
        match self {
            WorkerError::Unknown(message) => format!("{message}{error_logs}"),
            WorkerError::InvalidRequest(message) => format!("{message}{error_logs}"),
            WorkerError::StackOverflow => format!("Stack overflow{error_logs}"),
            WorkerError::OutOfMemory => format!("Out of memory{error_logs}"),
        }
    }
}

#[cfg(feature = "protobuf")]
mod protobuf {
    use crate::model::oplog::IndexedResourceKey;

    impl From<IndexedResourceKey> for golem_api_grpc::proto::golem::worker::IndexedResourceMetadata {
        fn from(value: IndexedResourceKey) -> Self {
            golem_api_grpc::proto::golem::worker::IndexedResourceMetadata {
                resource_name: value.resource_name,
                resource_params: value.resource_params,
            }
        }
    }

    impl From<golem_api_grpc::proto::golem::worker::IndexedResourceMetadata> for IndexedResourceKey {
        fn from(value: golem_api_grpc::proto::golem::worker::IndexedResourceMetadata) -> Self {
            IndexedResourceKey {
                resource_name: value.resource_name,
                resource_params: value.resource_params,
            }
        }
    }
}
