use std::fmt::{Display, Formatter};

use bincode::de::read::Reader;
use bincode::de::{BorrowDecoder, Decoder};
use bincode::enc::write::Writer;
use bincode::enc::Encoder;
use bincode::error::{DecodeError, EncodeError};
use bincode::{BorrowDecode, Decode, Encode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::RetryConfig;
use crate::model::regions::OplogRegion;
use crate::model::{
    AccountId, CallingConvention, ComponentVersion, IdempotencyKey, Timestamp, WorkerId,
    WorkerInvocation,
};

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

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum OplogEntry {
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
    },
    /// The worker invoked a host function
    ImportedFunctionInvoked {
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
        calling_convention: Option<CallingConvention>,
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
    SuccessfulUpdate {
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
    ) -> OplogEntry {
        OplogEntry::SuccessfulUpdate {
            timestamp: Timestamp::now_utc(),
            target_version,
            new_component_size,
        }
    }

    pub fn failed_update(target_version: ComponentVersion, details: Option<String>) -> OplogEntry {
        OplogEntry::FailedUpdate {
            timestamp: Timestamp::now_utc(),
            target_version,
            details,
        }
    }

    pub fn is_end_atomic_region(&self, idx: OplogIndex) -> bool {
        matches!(self, OplogEntry::EndAtomicRegion { begin_index, .. } if *begin_index == idx)
    }

    pub fn is_end_remote_write(&self, idx: OplogIndex) -> bool {
        matches!(self, OplogEntry::EndRemoteWrite { begin_index, .. } if *begin_index == idx)
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
        )
    }

    pub fn timestamp(&self) -> Timestamp {
        match self {
            OplogEntry::Create { timestamp, .. }
            | OplogEntry::ImportedFunctionInvoked { timestamp, .. }
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
            | OplogEntry::FailedUpdate { timestamp, .. } => *timestamp,
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
    ReadLocal,
    WriteLocal,
    ReadRemote,
    WriteRemote,
}

/// Describes the error that occurred in the worker
#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub enum WorkerError {
    Unknown(String),
    StackOverflow,
}

impl Display for WorkerError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerError::Unknown(message) => write!(f, "{}", message),
            WorkerError::StackOverflow => write!(f, "Stack overflow"),
        }
    }
}
