use crate::config::RetryConfig;
use crate::model::regions::OplogRegion;
use crate::model::{CallingConvention, InvocationKey, PromiseId, Timestamp};
use crate::serialization::{
    deserialize_with_version, serialize, try_deserialize, SERIALIZATION_VERSION_V1,
};
use bincode::{Decode, Encode};
use bytes::Bytes;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Encode, Decode)]
pub enum OplogEntry {
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
        invocation_key: Option<InvocationKey>,
        calling_convention: Option<CallingConvention>,
    },
    /// The worker has completed an invocation
    ExportedFunctionCompleted {
        timestamp: Timestamp,
        response: Vec<u8>,
        consumed_fuel: i64,
    },
    /// Promise created
    CreatePromise {
        timestamp: Timestamp,
        promise_id: PromiseId,
    },
    /// Promise completed
    CompletePromise {
        timestamp: Timestamp,
        promise_id: PromiseId,
        data: Vec<u8>,
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
        begin_index: u64,
    },
}

impl OplogEntry {
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
        invocation_key: Option<InvocationKey>,
        calling_convention: Option<CallingConvention>,
    ) -> Result<OplogEntry, String> {
        let serialized_request = serialize(request)?.to_vec();
        Ok(OplogEntry::ExportedFunctionInvoked {
            timestamp: Timestamp::now_utc(),
            function_name,
            request: serialized_request,
            invocation_key,
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

    pub fn create_promise(promise_id: PromiseId) -> OplogEntry {
        OplogEntry::CreatePromise {
            timestamp: Timestamp::now_utc(),
            promise_id,
        }
    }

    pub fn complete_promise(promise_id: PromiseId, data: Vec<u8>) -> OplogEntry {
        OplogEntry::CompletePromise {
            timestamp: Timestamp::now_utc(),
            promise_id,
            data,
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

    /// True if the oplog entry is a "hint" that should be skipped during replay
    pub fn is_hint(&self) -> bool {
        matches!(
            self,
            OplogEntry::Suspend { .. }
                | OplogEntry::Error { .. }
                | OplogEntry::Interrupted { .. }
                | OplogEntry::Exited { .. }
        )
    }

    pub fn response<T: DeserializeOwned + Decode>(&self) -> Result<Option<T>, String> {
        match &self {
            OplogEntry::ImportedFunctionInvoked { response, .. } => {
                let response_bytes: Bytes = Bytes::copy_from_slice(response);

                // In the v1 serialization format we did not have version prefix in the payloads.
                // We can assume though that if the payload starts with 2, it is serialized with the
                // v2 format because neither JSON nor protobuf (the two payload formats used in v1 for payloads)
                // start with 2 (This was verified with a simple test ValProtobufPrefixByteValidation).
                // So if the first byte is not 1 or 2 we assume it is a v1 format and deserialize it as JSON.
                match try_deserialize(&response_bytes)? {
                    Some(result) => Ok(Some(result)),
                    None => Ok(Some(deserialize_with_version(
                        &response_bytes,
                        SERIALIZATION_VERSION_V1,
                    )?)),
                }
            }
            OplogEntry::ExportedFunctionCompleted { response, .. } => {
                let response_bytes: Bytes = Bytes::copy_from_slice(response);

                // See the comment above for the explanation of this logic
                match try_deserialize(&response_bytes)? {
                    Some(result) => Ok(Some(result)),
                    None => Ok(Some(deserialize_with_version(
                        &response_bytes,
                        SERIALIZATION_VERSION_V1,
                    )?)),
                }
            }
            _ => Ok(None),
        }
    }

    pub fn payload_as_val_array(
        &self,
    ) -> Result<Option<Vec<golem_wasm_rpc::protobuf::Val>>, String> {
        // This is a special case of a possible generic request() accessor, because in v1 the only
        // data type we serialized was Vec<Val> and it was done in a special way (every element serialized
        // via protobuf separately, then an array of byte arrays serialized into JSON)
        match &self {
            OplogEntry::ExportedFunctionInvoked {
                function_name,
                request,
                ..
            } => {
                let request_bytes: Bytes = Bytes::copy_from_slice(request);
                self.try_decode_val_array_payload(function_name, &request_bytes)
            }
            OplogEntry::ExportedFunctionCompleted { response, .. } => {
                let response_bytes: Bytes = Bytes::copy_from_slice(response);
                self.try_decode_val_array_payload("?", &response_bytes)
            }
            _ => Ok(None),
        }
    }

    fn try_decode_val_array_payload(
        &self,
        function_name: &str,
        payload: &Bytes,
    ) -> Result<Option<Vec<golem_wasm_rpc::protobuf::Val>>, String> {
        match try_deserialize(payload)? {
            Some(result) => Ok(Some(result)),
            None => {
                let deserialized_array: Vec<Vec<u8>> = serde_json::from_slice(payload)
                    .unwrap_or_else(|err| {
                        panic!(
                            "Failed to deserialize oplog payload: {:?}: {err}",
                            std::str::from_utf8(payload).unwrap_or("???")
                        )
                    });
                let function_input = deserialized_array
                    .iter()
                    .map(|serialized_value| {
                        <golem_wasm_rpc::protobuf::Val as prost::Message>::decode(serialized_value.as_slice())
                            .unwrap_or_else(|err| panic!("Failed to deserialize function input {:?} for {function_name}: {err}", serialized_value))
                    })
                    .collect::<Vec<golem_wasm_rpc::protobuf::Val>>();
                Ok(Some(function_input))
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
pub enum WrappedFunctionType {
    ReadLocal,
    WriteLocal,
    ReadRemote,
    WriteRemote,
}

/// Describes the error that occurred in the worker
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Encode, Decode)]
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
    use super::{OplogEntry, WrappedFunctionType};
    use crate::model::{CallingConvention, InvocationKey, Timestamp};
    use golem_wasm_rpc::protobuf::{val, Val, ValResult};

    #[test]
    fn oplog_entry_imported_function_invoked_payload_roundtrip() {
        let timestamp = Timestamp::now_utc();
        let entry = OplogEntry::imported_function_invoked(
            timestamp,
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

        let response = entry.response::<String>().unwrap().unwrap();

        assert_eq!(response, "example payload");
    }

    #[test]
    fn oplog_entry_imported_function_invoked_payload_v1() {
        let timestamp = Timestamp::now_utc();
        let entry = OplogEntry::ImportedFunctionInvoked {
            timestamp,
            function_name: "function_name".to_string(),
            response: serde_json::to_vec("example payload").unwrap(),
            wrapped_function_type: WrappedFunctionType::ReadLocal,
        };

        let response = entry.response::<String>().unwrap().unwrap();

        assert_eq!(response, "example payload");
    }

    #[test]
    fn oplog_entry_exported_function_invoked_payload_roundtrip() {
        let timestamp = Timestamp::now_utc();

        let val1 = Val {
            val: Some(val::Val::Result(Box::new(ValResult {
                discriminant: 0,
                value: Some(Box::new(Val {
                    val: Some(val::Val::U64(10)),
                })),
            }))),
        };
        let entry = OplogEntry::exported_function_invoked(
            timestamp,
            "function_name".to_string(),
            &vec![val1.clone()],
            Some(InvocationKey {
                value: "invocation_key".to_string(),
            }),
            Some(CallingConvention::Stdio),
        )
        .unwrap();

        if let OplogEntry::ExportedFunctionInvoked { request, .. } = &entry {
            assert_eq!(request.len(), 9);
        } else {
            unreachable!()
        }

        let request = entry.payload_as_val_array().unwrap().unwrap();

        assert_eq!(request, vec![val1]);
    }

    #[test]
    fn oplog_entry_exported_function_invoked_payload_v1() {
        let timestamp = Timestamp::now_utc();

        let val1 = Val {
            val: Some(val::Val::Result(Box::new(ValResult {
                discriminant: 0,
                value: Some(Box::new(Val {
                    val: Some(val::Val::U64(10)),
                })),
            }))),
        };
        let val1_bytes = prost::Message::encode_to_vec(&val1);
        let request_bytes = serde_json::to_vec(&vec![val1_bytes]).unwrap();

        let entry = OplogEntry::ExportedFunctionInvoked {
            timestamp,
            function_name: "function_name".to_string(),
            request: request_bytes,
            invocation_key: Some(InvocationKey {
                value: "invocation_key".to_string(),
            }),
            calling_convention: Some(CallingConvention::Stdio),
        };

        let request = entry.payload_as_val_array().unwrap().unwrap();
        assert_eq!(request, vec![val1]);
    }

    #[test]
    fn oplog_entry_exported_function_completed_roundtrip() {
        let timestamp = Timestamp::now_utc();

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
            timestamp,
            &vec![val1.clone(), val2.clone()],
            1_000_000_000,
        )
        .unwrap();

        if let OplogEntry::ExportedFunctionCompleted { response, .. } = &entry {
            assert_eq!(response.len(), 21);
        } else {
            unreachable!()
        }

        let response = entry.payload_as_val_array().unwrap().unwrap();

        assert_eq!(response, vec![val1, val2]);
    }

    #[test]
    fn oplog_entry_exported_function_completed_v1() {
        let timestamp = Timestamp::now_utc();

        let val1 = Val {
            val: Some(val::Val::Result(Box::new(ValResult {
                discriminant: 0,
                value: Some(Box::new(Val {
                    val: Some(val::Val::U64(10)),
                })),
            }))),
        };
        let val1_bytes = prost::Message::encode_to_vec(&val1);
        let val2 = Val {
            val: Some(val::Val::String("something".to_string())),
        };
        let val2_bytes = prost::Message::encode_to_vec(&val2);

        let response_bytes = serde_json::to_vec(&vec![val1_bytes, val2_bytes]).unwrap();

        let entry = OplogEntry::ExportedFunctionCompleted {
            timestamp,
            response: response_bytes,
            consumed_fuel: 1_000_000_000,
        };

        let response = entry.payload_as_val_array().unwrap().unwrap();

        assert_eq!(response, vec![val1, val2]);
    }
}
