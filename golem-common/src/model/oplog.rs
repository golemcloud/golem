use std::fmt::{Display, Formatter};

use bincode::{Decode, Encode};
use bytes::Bytes;

use crate::config::RetryConfig;
use crate::model::regions::OplogRegion;
use crate::model::{
    AccountId, CallingConvention, ComponentVersion, IdempotencyKey, Timestamp, WorkerId,
    WorkerInvocation,
};
use crate::serialization::{serialize, try_deserialize};

pub type OplogIndex = u64;

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub enum OplogEntry {
    Create {
        timestamp: Timestamp,
        worker_id: WorkerId,
        component_version: ComponentVersion,
        args: Vec<String>,
        env: Vec<(String, String)>,
        account_id: AccountId,
    },
    /// The worker invoked a host function
    ImportedFunctionInvoked {
        timestamp: Timestamp,
        function_name: String,
        response: Vec<u8>,
        wrapped_function_type: WrappedFunctionType,
    },
    /// The worker has been invoked
    ExportedFunctionInvoked {
        timestamp: Timestamp,
        function_name: String,
        request: Vec<u8>,
        idempotency_key: IdempotencyKey,
        calling_convention: Option<CallingConvention>,
    },
    /// The worker has completed an invocation
    ExportedFunctionCompleted {
        timestamp: Timestamp,
        response: Vec<u8>,
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
    ) -> OplogEntry {
        OplogEntry::Create {
            timestamp: Timestamp::now_utc(),
            worker_id,
            component_version,
            args,
            env,
            account_id,
        }
    }

    pub fn imported_function_invoked<R: Encode>(
        function_name: String,
        response: &R,
        wrapped_function_type: WrappedFunctionType,
    ) -> Result<OplogEntry, String> {
        let serialized_response = serialize(response)?.to_vec();

        Ok(OplogEntry::ImportedFunctionInvoked {
            timestamp: Timestamp::now_utc(),
            function_name,
            response: serialized_response,
            wrapped_function_type,
        })
    }

    pub fn exported_function_invoked<R: Encode>(
        function_name: String,
        request: &R,
        idempotency_key: IdempotencyKey,
        calling_convention: Option<CallingConvention>,
    ) -> Result<OplogEntry, String> {
        let serialized_request = serialize(request)?.to_vec();
        Ok(OplogEntry::ExportedFunctionInvoked {
            timestamp: Timestamp::now_utc(),
            function_name,
            request: serialized_request,
            idempotency_key,
            calling_convention,
        })
    }

    pub fn exported_function_completed<R: Encode>(
        response: &R,
        consumed_fuel: i64,
    ) -> Result<OplogEntry, String> {
        let serialized_response = serialize(response)?.to_vec();
        Ok(OplogEntry::ExportedFunctionCompleted {
            timestamp: Timestamp::now_utc(),
            response: serialized_response,
            consumed_fuel,
        })
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

    pub fn end_atomic_region(begin_index: u64) -> OplogEntry {
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

    pub fn successful_update(target_version: ComponentVersion) -> OplogEntry {
        OplogEntry::SuccessfulUpdate {
            timestamp: Timestamp::now_utc(),
            target_version,
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

    pub fn payload<T: Decode>(&self) -> Result<Option<T>, String> {
        match &self {
            OplogEntry::ImportedFunctionInvoked { response, .. } => {
                let response_bytes: Bytes = Bytes::copy_from_slice(response);
                try_deserialize(&response_bytes)
            }
            OplogEntry::ExportedFunctionInvoked { request, .. } => {
                let response_bytes: Bytes = Bytes::copy_from_slice(request);
                try_deserialize(&response_bytes)
            }
            OplogEntry::ExportedFunctionCompleted { response, .. } => {
                let response_bytes: Bytes = Bytes::copy_from_slice(response);
                try_deserialize(&response_bytes)
            }
            _ => Ok(None),
        }
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
        source: SnapshotSource,
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
pub enum SnapshotSource {
    /// Load the snapshot from the given byte array
    Inline(Vec<u8>),

    /// Load the snapshot from the blob store
    BlobStore {
        account_id: AccountId,
        container: String,
        object: String,
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

#[cfg(test)]
mod tests {
    use golem_wasm_rpc::protobuf::{val, Val, ValResult};

    use crate::model::{CallingConvention, IdempotencyKey};

    use super::{OplogEntry, WrappedFunctionType};

    #[test]
    fn oplog_entry_imported_function_invoked_payload_roundtrip() {
        let entry = OplogEntry::imported_function_invoked(
            "function_name".to_string(),
            &("example payload".to_string()),
            WrappedFunctionType::ReadLocal,
        )
        .unwrap();

        if let OplogEntry::ImportedFunctionInvoked { response, .. } = &entry {
            assert_eq!(response.len(), 17);
        } else {
            unreachable!()
        }

        let response = entry.payload::<String>().unwrap().unwrap();

        assert_eq!(response, "example payload");
    }

    #[test]
    fn oplog_entry_exported_function_invoked_payload_roundtrip() {
        let val1 = Val {
            val: Some(val::Val::Result(Box::new(ValResult {
                discriminant: 0,
                value: Some(Box::new(Val {
                    val: Some(val::Val::U64(10)),
                })),
            }))),
        };
        let entry = OplogEntry::exported_function_invoked(
            "function_name".to_string(),
            &vec![val1.clone()],
            IdempotencyKey {
                value: "idempotency-key".to_string(),
            },
            Some(CallingConvention::Stdio),
        )
        .unwrap();

        if let OplogEntry::ExportedFunctionInvoked { request, .. } = &entry {
            assert_eq!(request.len(), 9);
        } else {
            unreachable!()
        }

        let request: Vec<Val> = entry.payload().unwrap().unwrap();

        assert_eq!(request, vec![val1]);
    }

    #[test]
    fn oplog_entry_exported_function_completed_roundtrip() {
        let val1 = Val {
            val: Some(val::Val::Result(Box::new(ValResult {
                discriminant: 0,
                value: Some(Box::new(Val {
                    val: Some(val::Val::U64(10)),
                })),
            }))),
        };
        let val2 = Val {
            val: Some(val::Val::String("something".to_string())),
        };

        let entry = OplogEntry::exported_function_completed(
            &vec![val1.clone(), val2.clone()],
            1_000_000_000,
        )
        .unwrap();

        if let OplogEntry::ExportedFunctionCompleted { response, .. } = &entry {
            assert_eq!(response.len(), 21);
        } else {
            unreachable!()
        }

        let response: Vec<Val> = entry.payload().unwrap().unwrap();

        assert_eq!(response, vec![val1, val2]);
    }
}
