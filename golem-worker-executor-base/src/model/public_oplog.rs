// Copyright 2024 Golem Cloud
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

use crate::durable_host::http::serialized::{
    SerializableErrorCode, SerializableResponse, SerializableResponseHeaders,
};
use crate::durable_host::serialized::{
    SerializableDateTime, SerializableError, SerializableFileTimes, SerializableIpAddresses,
    SerializableStreamError,
};
use crate::durable_host::wasm_rpc::serialized::SerializableInvokeResult;
use crate::error::GolemError;
use crate::services::oplog::Oplog;
use crate::services::rpc::RpcError;
use crate::services::worker_proxy::WorkerProxyError;
use bincode::Decode;
use golem_common::config::RetryConfig;
use golem_common::model::oplog::{
    IndexedResourceKey, LogLevel, OplogEntry, OplogIndex, UpdateDescription, WorkerError,
    WorkerResourceId, WrappedFunctionType,
};
use golem_common::model::regions::OplogRegion;
use golem_common::model::{
    AccountId, ComponentVersion, IdempotencyKey, Timestamp, WorkerId, WorkerInvocation,
};
use golem_common::serialization::try_deserialize;
use golem_wasm_ast::analysis::{
    AnalysedType, NameOptionTypePair, TypeBool, TypeList, TypeOption, TypeResult, TypeStr,
    TypeTuple, TypeU32, TypeU64, TypeU8, TypeVariant,
};
use golem_wasm_rpc::{Value, WitValue};
use std::collections::HashMap;
use std::sync::Arc;
use uuid::Uuid;

/// A mirror of the core `OplogEntry` type, without the undefined arbitrary payloads.
///
/// Instead it encodes all payloads with wasm-rpc `Value` types. This makes this the base type
/// for exposing oplog entries through various APIs such as gRPC, REST and WIT.
///
/// The rest of the system will always use `OplogEntry` internally - the only point where the
/// oplog payloads are decoded and re-encoded as `Value` is in this module and it should only be used
/// before exposing an oplog entry through a public API.
#[derive(Clone, Debug)]
pub enum PublicOplogEntry {
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
        response: ValueAndType,
        wrapped_function_type: WrappedFunctionType,
    },
    /// The worker has been invoked
    ExportedFunctionInvoked {
        timestamp: Timestamp,
        function_name: String,
        request: Vec<ValueAndType>,
        idempotency_key: IdempotencyKey,
    },
    /// The worker has completed an invocation
    ExportedFunctionCompleted {
        timestamp: Timestamp,
        response: ValueAndType,
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
}

impl PublicOplogEntry {
    // TODO: need access to component metadata cache, and a "current component version" which need to be stored in the cursor
    pub async fn from_oplog_entry(
        value: OplogEntry,
        oplog: Arc<dyn Oplog + Send + Sync>,
    ) -> Result<Self, String> {
        match value {
            OplogEntry::Create {
                timestamp,
                worker_id,
                component_version,
                args,
                env,
                account_id,
                parent,
                component_size,
                initial_total_linear_memory_size,
            } => Ok(PublicOplogEntry::Create {
                timestamp,
                worker_id,
                component_version,
                args,
                env,
                account_id,
                parent,
                component_size,
                initial_total_linear_memory_size,
            }),
            OplogEntry::ImportedFunctionInvoked {
                timestamp,
                function_name,
                response,
                wrapped_function_type,
            } => {
                let payload_bytes = oplog.download_payload(&response).await?;
                let value =
                    Self::encode_host_function_response_as_value(&function_name, &payload_bytes)?;
                Ok(PublicOplogEntry::ImportedFunctionInvoked {
                    timestamp,
                    function_name,
                    response: value,
                    wrapped_function_type,
                })
            }
            OplogEntry::ExportedFunctionInvoked {
                timestamp,
                function_name,
                request,
                idempotency_key,
            } => {
                let payload_bytes = oplog.download_payload(&request).await?;
                let proto_params: Vec<golem_wasm_rpc::protobuf::Val> =
                    try_deserialize(&payload_bytes)?.unwrap_or_default();
                let _params = proto_params
                    .into_iter()
                    .map(Value::try_from)
                    .collect::<Result<Vec<_>, _>>()?;

                // TODO: need to get type info

                Ok(PublicOplogEntry::ExportedFunctionInvoked {
                    timestamp,
                    function_name,
                    request: Vec::new(), // TODO
                    idempotency_key,
                })
            }
            OplogEntry::ExportedFunctionCompleted {
                timestamp,
                response,
                consumed_fuel,
            } => {
                let payload_bytes = oplog.download_payload(&response).await?;
                let proto_type_annotated_value: golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue =
                    try_deserialize(&payload_bytes)?.unwrap_or(
                        golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue::Record(
                            golem_wasm_rpc::protobuf::TypedRecord {
                                typ: Vec::new(),
                                value: Vec::new(),
                            },
                        ),
                    );
                let typ: AnalysedType = AnalysedType::try_from(&proto_type_annotated_value)?;
                let value = Value::try_from(proto_type_annotated_value)?;
                Ok(PublicOplogEntry::ExportedFunctionCompleted {
                    timestamp,
                    response: ValueAndType::new(value, typ),
                    consumed_fuel,
                })
            }
            OplogEntry::Suspend { timestamp } => Ok(PublicOplogEntry::Suspend { timestamp }),
            OplogEntry::Error { timestamp, error } => {
                Ok(PublicOplogEntry::Error { timestamp, error })
            }
            OplogEntry::NoOp { timestamp } => Ok(PublicOplogEntry::NoOp { timestamp }),
            OplogEntry::Jump { timestamp, jump } => Ok(PublicOplogEntry::Jump { timestamp, jump }),
            OplogEntry::Interrupted { timestamp } => {
                Ok(PublicOplogEntry::Interrupted { timestamp })
            }
            OplogEntry::Exited { timestamp } => Ok(PublicOplogEntry::Exited { timestamp }),
            OplogEntry::ChangeRetryPolicy {
                timestamp,
                new_policy,
            } => Ok(PublicOplogEntry::ChangeRetryPolicy {
                timestamp,
                new_policy,
            }),
            OplogEntry::BeginAtomicRegion { timestamp } => {
                Ok(PublicOplogEntry::BeginAtomicRegion { timestamp })
            }
            OplogEntry::EndAtomicRegion {
                timestamp,
                begin_index,
            } => Ok(PublicOplogEntry::EndAtomicRegion {
                timestamp,
                begin_index,
            }),
            OplogEntry::BeginRemoteWrite { timestamp } => {
                Ok(PublicOplogEntry::BeginRemoteWrite { timestamp })
            }
            OplogEntry::EndRemoteWrite {
                timestamp,
                begin_index,
            } => Ok(PublicOplogEntry::EndRemoteWrite {
                timestamp,
                begin_index,
            }),
            OplogEntry::PendingWorkerInvocation {
                timestamp,
                invocation,
            } => Ok(PublicOplogEntry::PendingWorkerInvocation {
                timestamp,
                invocation,
            }),
            OplogEntry::PendingUpdate {
                timestamp,
                description,
            } => Ok(PublicOplogEntry::PendingUpdate {
                timestamp,
                description,
            }),
            OplogEntry::SuccessfulUpdate {
                timestamp,
                target_version,
                new_component_size,
            } => Ok(PublicOplogEntry::SuccessfulUpdate {
                timestamp,
                target_version,
                new_component_size,
            }),
            OplogEntry::FailedUpdate {
                timestamp,
                target_version,
                details,
            } => Ok(PublicOplogEntry::FailedUpdate {
                timestamp,
                target_version,
                details,
            }),
            OplogEntry::GrowMemory { timestamp, delta } => {
                Ok(PublicOplogEntry::GrowMemory { timestamp, delta })
            }
            OplogEntry::CreateResource { timestamp, id } => {
                Ok(PublicOplogEntry::CreateResource { timestamp, id })
            }
            OplogEntry::DropResource { timestamp, id } => {
                Ok(PublicOplogEntry::DropResource { timestamp, id })
            }
            OplogEntry::DescribeResource {
                timestamp,
                id,
                indexed_resource,
            } => Ok(PublicOplogEntry::DescribeResource {
                timestamp,
                id,
                indexed_resource,
            }),
            OplogEntry::Log {
                timestamp,
                level,
                context,
                message,
            } => Ok(PublicOplogEntry::Log {
                timestamp,
                level,
                context,
                message,
            }),
            OplogEntry::Restart { timestamp } => Ok(PublicOplogEntry::Restart { timestamp }),
        }
    }

    fn try_deserialize<T: Decode>(data: &[u8]) -> Result<T, String> {
        try_deserialize(data)?.ok_or("Unexpected oplog payload, cannot deserialize".to_string())
    }

    fn encode_host_function_response_as_value(
        function_name: &str,
        bytes: &[u8],
    ) -> Result<ValueAndType, String> {
        match function_name {
            "golem::rpc::future-invoke-result::get" => {
                let payload: SerializableInvokeResult = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "http::types::future_incoming_response::get" => {
                let payload: SerializableResponse = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem io::poll::poll" => {
                let payload: Result<Vec<u32>, SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem blobstore::container::object_info" => {
                let payload: Result<
                    crate::services::blob_store::ObjectMetadata,
                    SerializableError,
                > = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem blobstore::container::delete_objects" => {
                let payload: Result<(), SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem blobstore::container::list_objects" => {
                let payload: Result<Vec<String>, SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem blobstore::container::get_data" => {
                let payload: Result<Vec<u8>, SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem blobstore::container::write_data" => {
                let payload: Result<(), SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem blobstore::container::delete_object" => {
                let payload: Result<(), SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem blobstore::container::has_object" => {
                let payload: Result<bool, SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem blobstore::container::clear" => {
                let payload: Result<(), SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem blobstore::blobstore::copy_object" => {
                let payload: Result<(), SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem blobstore::blobstore::delete_container" => {
                let payload: Result<(), SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem blobstore::blobstore::create_container" => {
                let payload: Result<u64, SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem blobstore::blobstore::get_container" => {
                let payload: Result<Option<u64>, SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem blobstore::blobstore::container_exists" => {
                let payload: Result<bool, SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem blobstore::blobstore::move_object" => {
                let payload: Result<(), SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem_environment::get_arguments" => {
                let payload: Result<Vec<String>, SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem_environment::get_environment" => {
                let payload: Result<Vec<(String, String)>, SerializableError> =
                    Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem_environment::initial_cwd" => {
                let payload: Result<Option<String>, SerializableError> =
                    Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "monotonic_clock::resolution" => {
                let payload: Result<u64, SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "monotonic_clock::now" => {
                let payload: Result<u64, SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "monotonic_clock::subscribe_duration" => {
                let payload: Result<u64, SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "wall_clock::now" => {
                let payload: Result<SerializableDateTime, SerializableError> =
                    Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "wall_clock::resolution" => {
                let payload: Result<SerializableDateTime, SerializableError> =
                    Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem_delete_promise" => {
                let payload: Result<(), SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem_complete_promise" => {
                let payload: Result<bool, SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem::api::update-worker" => {
                let payload: Result<(), SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "http::types::incoming_body_stream::skip" => {
                let payload: Result<u64, SerializableStreamError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "http::types::incoming_body_stream::read" => {
                let payload: Result<Vec<u8>, SerializableStreamError> =
                    Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "http::types::incoming_body_stream::blocking_read" => {
                let payload: Result<Vec<u8>, SerializableStreamError> =
                    Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "http::types::incoming_body_stream::blocking_skip" => {
                let payload: Result<u64, SerializableStreamError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem keyvalue::eventual::delete" => {
                let payload: Result<(), SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem keyvalue::eventual::get" => {
                let payload: Result<Option<Vec<u8>>, SerializableError> =
                    Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem keyvalue::eventual::set" => {
                let payload: Result<(), SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem keyvalue::eventual::exists" => {
                let payload: Result<bool, SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem keyvalue::eventual_batch::set_many" => {
                let payload: Result<(), SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem keyvalue::eventual_batch::get_many" => {
                let payload: Result<Vec<Option<Vec<u8>>>, SerializableError> =
                    Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem keyvalue::eventual_batch::get_keys" => {
                let payload: Result<Vec<String>, SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem keyvalue::eventual_batch::delete_many" => {
                let payload: Result<(), SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem random::insecure::get_insecure_random_bytes" => {
                let payload: Result<Vec<u8>, SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem random::insecure::get_insecure_random_u64" => {
                let payload: Result<u64, SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem random::insecure_seed::insecure_seed" => {
                let payload: Result<(u64, u64), SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem random::get_random_bytes" => {
                let payload: Result<Vec<u8>, SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem random::get_random_u64" => {
                let payload: Result<u64, SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "sockets::ip_name_lookup::resolve_addresses" => {
                let payload: Result<SerializableIpAddresses, SerializableError> =
                    Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem::rpc::wasm-rpc::invoke" => {
                let payload: Result<(), SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem::rpc::wasm-rpc::invoke-and-await" => {
                let payload: Result<WitValue, SerializableError> = Self::try_deserialize(bytes)?;
                // TODO: need type info which we can get if we know the target component id and version
                todo!()
            }
            "golem::rpc::wasm-rpc::generate_unique_local_worker_id" => {
                let payload: Result<WorkerId, SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "cli::preopens::get_directories" => {
                let payload: Result<Vec<String>, SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "filesystem::types::descriptor::stat" => {
                let payload: Result<SerializableFileTimes, SerializableError> =
                    Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "filesystem::types::descriptor::stat_at" => {
                let payload: Result<SerializableFileTimes, SerializableError> =
                    Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem api::generate_idempotency_key" => {
                let payload =
                    Self::try_deserialize::<Result<(u64, u64), SerializableError>>(bytes)?
                        .map(|pair| Uuid::from_u64_pair(pair.0, pair.1));
                Ok(payload.into_value_and_type())
            }
            "golem http::types::future_trailers::get" => {
                let payload: Result<
                    Option<
                        Result<Result<Option<HashMap<String, Vec<u8>>>, SerializableErrorCode>, ()>,
                    >,
                    SerializableError,
                > = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem::rpc::wasm-rpc::invoke idempotency key" => {
                let payload =
                    Self::try_deserialize::<Result<(u64, u64), SerializableError>>(bytes)?
                        .map(|pair| Uuid::from_u64_pair(pair.0, pair.1));
                Ok(payload.into_value_and_type())
            }
            "golem::rpc::wasm-rpc::invoke-and-await idempotency key" => {
                let payload =
                    Self::try_deserialize::<Result<(u64, u64), SerializableError>>(bytes)?
                        .map(|pair| Uuid::from_u64_pair(pair.0, pair.1));
                Ok(payload.into_value_and_type())
            }
            "golem::rpc::wasm-rpc::async-invoke-and-await idempotency key" => {
                let payload =
                    Self::try_deserialize::<Result<(u64, u64), SerializableError>>(bytes)?
                        .map(|pair| Uuid::from_u64_pair(pair.0, pair.1));
                Ok(payload.into_value_and_type())
            }
            _ => Err(format!("Unsupported host function name: {}", function_name)),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ValueAndType {
    pub value: Value,
    pub typ: AnalysedType,
}

impl ValueAndType {
    pub fn new(value: Value, typ: AnalysedType) -> Self {
        Self { value, typ }
    }
}

/// Specific trait to convert a type into a `Value` type.
///
/// The reason for not using Into<Value> instead is to limit the scope of these conversions
/// into this module.
trait IntoValue {
    fn into_value(self) -> Value;
    fn get_type() -> AnalysedType;

    fn into_value_and_type(self) -> ValueAndType
    where
        Self: Sized,
    {
        ValueAndType::new(self.into_value(), Self::get_type())
    }
}

impl IntoValue for u8 {
    fn into_value(self) -> Value {
        Value::U8(self)
    }

    fn get_type() -> AnalysedType {
        AnalysedType::U8(TypeU8)
    }
}

impl IntoValue for u32 {
    fn into_value(self) -> Value {
        Value::U32(self)
    }

    fn get_type() -> AnalysedType {
        AnalysedType::U32(TypeU32)
    }
}

impl IntoValue for u64 {
    fn into_value(self) -> Value {
        Value::U64(self)
    }

    fn get_type() -> AnalysedType {
        AnalysedType::U64(TypeU64)
    }
}

impl IntoValue for bool {
    fn into_value(self) -> Value {
        Value::Bool(self)
    }

    fn get_type() -> AnalysedType {
        AnalysedType::Bool(TypeBool)
    }
}

impl IntoValue for String {
    fn into_value(self) -> Value {
        Value::String(self)
    }

    fn get_type() -> AnalysedType {
        AnalysedType::Str(TypeStr)
    }
}

impl<S: IntoValue, E: IntoValue> IntoValue for Result<S, E> {
    fn into_value(self) -> Value {
        match self {
            Ok(s) => Value::Result(Ok(Some(Box::new(s.into_value())))),
            Err(e) => Value::Result(Err(Some(Box::new(e.into_value())))),
        }
    }

    fn get_type() -> AnalysedType {
        AnalysedType::Result(TypeResult {
            ok: Some(Box::new(S::get_type())),
            err: Some(Box::new(E::get_type())),
        })
    }
}

impl<E: IntoValue> IntoValue for Result<(), E> {
    fn into_value(self) -> Value {
        match self {
            Ok(_) => Value::Result(Ok(None)),
            Err(e) => Value::Result(Err(Some(Box::new(e.into_value())))),
        }
    }

    fn get_type() -> AnalysedType {
        AnalysedType::Result(TypeResult {
            ok: None,
            err: Some(Box::new(E::get_type())),
        })
    }
}

impl<S: IntoValue> IntoValue for Result<S, ()> {
    fn into_value(self) -> Value {
        match self {
            Ok(s) => Value::Result(Ok(Some(Box::new(s.into_value())))),
            Err(_) => Value::Result(Err(None)),
        }
    }

    fn get_type() -> AnalysedType {
        AnalysedType::Result(TypeResult {
            ok: Some(Box::new(S::get_type())),
            err: None,
        })
    }
}

impl<T: IntoValue> IntoValue for Option<T> {
    fn into_value(self) -> Value {
        match self {
            Some(t) => Value::Option(Some(Box::new(t.into_value()))),
            None => Value::Option(None),
        }
    }

    fn get_type() -> AnalysedType {
        AnalysedType::Option(TypeOption {
            inner: Box::new(T::get_type()),
        })
    }
}

impl<T: IntoValue> IntoValue for Vec<T> {
    fn into_value(self) -> Value {
        Value::List(self.into_iter().map(IntoValue::into_value).collect())
    }

    fn get_type() -> AnalysedType {
        AnalysedType::List(TypeList {
            inner: Box::new(T::get_type()),
        })
    }
}

impl<A: IntoValue, B: IntoValue> IntoValue for (A, B) {
    fn into_value(self) -> Value {
        Value::Tuple(vec![self.0.into_value(), self.1.into_value()])
    }

    fn get_type() -> AnalysedType {
        AnalysedType::Tuple(TypeTuple {
            items: vec![A::get_type(), B::get_type()],
        })
    }
}

impl<K: IntoValue, V: IntoValue> IntoValue for HashMap<K, V> {
    fn into_value(self) -> Value {
        Value::List(
            self.into_iter()
                .map(|(k, v)| Value::Tuple(vec![k.into_value(), v.into_value()]))
                .collect(),
        )
    }

    fn get_type() -> AnalysedType {
        AnalysedType::List(TypeList {
            inner: Box::new(AnalysedType::Tuple(TypeTuple {
                items: vec![K::get_type(), V::get_type()],
            })),
        })
    }
}

impl IntoValue for SerializableInvokeResult {
    fn into_value(self) -> Value {
        match self {
            SerializableInvokeResult::Failed(serializable_error) => Value::Variant {
                case_idx: 0,
                case_value: Some(Box::new(serializable_error.into_value())),
            },
            SerializableInvokeResult::Pending => Value::Variant {
                case_idx: 1,
                case_value: None,
            },
            SerializableInvokeResult::Completed(result) => Value::Variant {
                case_idx: 2,
                case_value: Some(Box::new(result.into_value())),
            },
        }
    }

    fn get_type() -> AnalysedType {
        AnalysedType::Variant(TypeVariant {
            cases: vec![
                NameOptionTypePair {
                    name: "Failed".to_string(),
                    typ: Some(SerializableError::get_type()),
                },
                NameOptionTypePair {
                    name: "Pending".to_string(),
                    typ: None,
                },
                NameOptionTypePair {
                    name: "Completed".to_string(),
                    typ: Some(Result::<WitValue, RpcError>::get_type()),
                },
            ],
        })
    }
}

impl IntoValue for SerializableError {
    fn into_value(self) -> Value {
        match self {
            SerializableError::Generic { message } => Value::Variant {
                case_idx: 0,
                case_value: Some(Box::new(message.into_value())),
            },
            SerializableError::FsError { code } => Value::Variant {
                case_idx: 1,
                case_value: Some(Box::new(code.into_value())),
            },
            SerializableError::Golem { error } => Value::Variant {
                case_idx: 2,
                case_value: Some(Box::new(error.into_value())),
            },
            SerializableError::SocketError { code } => Value::Variant {
                case_idx: 3,
                case_value: Some(Box::new(code.into_value())),
            },
            SerializableError::Rpc { error } => Value::Variant {
                case_idx: 4,
                case_value: Some(Box::new(error.into_value())),
            },
            SerializableError::WorkerProxy { error } => Value::Variant {
                case_idx: 5,
                case_value: Some(Box::new(error.into_value())),
            },
        }
    }

    fn get_type() -> AnalysedType {
        AnalysedType::Variant(TypeVariant {
            cases: vec![
                NameOptionTypePair {
                    name: "Generic".to_string(),
                    typ: Some(String::get_type()),
                },
                NameOptionTypePair {
                    name: "FsError".to_string(),
                    typ: Some(u8::get_type()),
                },
                NameOptionTypePair {
                    name: "Golem".to_string(),
                    typ: Some(GolemError::get_type()),
                },
                NameOptionTypePair {
                    name: "SocketError".to_string(),
                    typ: Some(u8::get_type()),
                },
                NameOptionTypePair {
                    name: "Rpc".to_string(),
                    typ: Some(RpcError::get_type()),
                },
                NameOptionTypePair {
                    name: "WorkerProxy".to_string(),
                    typ: Some(WorkerProxyError::get_type()),
                },
            ],
        })
    }
}

impl IntoValue for SerializableStreamError {
    fn into_value(self) -> Value {
        todo!()
    }

    fn get_type() -> AnalysedType {
        todo!()
    }
}

impl IntoValue for WitValue {
    fn into_value(self) -> Value {
        self.into()
    }

    fn get_type() -> AnalysedType {
        todo!()
    }
}

impl IntoValue for RpcError {
    fn into_value(self) -> Value {
        todo!()
    }

    fn get_type() -> AnalysedType {
        todo!()
    }
}

impl IntoValue for SerializableResponse {
    fn into_value(self) -> Value {
        match self {
            SerializableResponse::Pending => Value::Variant {
                case_idx: 0,
                case_value: None,
            },
            SerializableResponse::HeadersReceived(headers) => Value::Variant {
                case_idx: 1,
                case_value: Some(Box::new(headers.into_value())),
            },
            SerializableResponse::HttpError(error_code) => Value::Variant {
                case_idx: 2,
                case_value: Some(Box::new(error_code.into_value())),
            },
            SerializableResponse::InternalError(error) => Value::Variant {
                case_idx: 3,
                case_value: Some(Box::new(error.into_value())),
            },
        }
    }

    fn get_type() -> AnalysedType {
        AnalysedType::Variant(TypeVariant {
            cases: vec![
                NameOptionTypePair {
                    name: "Pending".to_string(),
                    typ: None,
                },
                NameOptionTypePair {
                    name: "HeadersReceived".to_string(),
                    typ: Some(SerializableResponseHeaders::get_type()),
                },
                NameOptionTypePair {
                    name: "HttpError".to_string(),
                    typ: Some(SerializableErrorCode::get_type()),
                },
                NameOptionTypePair {
                    name: "InternalError".to_string(),
                    typ: Some(Option::<SerializableError>::get_type()),
                },
            ],
        })
    }
}

impl IntoValue for SerializableResponseHeaders {
    fn into_value(self) -> Value {
        todo!()
    }

    fn get_type() -> AnalysedType {
        todo!()
    }
}

impl IntoValue for SerializableErrorCode {
    fn into_value(self) -> Value {
        todo!()
    }

    fn get_type() -> AnalysedType {
        todo!()
    }
}

impl IntoValue for GolemError {
    fn into_value(self) -> Value {
        todo!()
    }

    fn get_type() -> AnalysedType {
        todo!()
    }
}

impl IntoValue for WorkerProxyError {
    fn into_value(self) -> Value {
        todo!()
    }

    fn get_type() -> AnalysedType {
        todo!()
    }
}

impl IntoValue for crate::services::blob_store::ObjectMetadata {
    fn into_value(self) -> Value {
        todo!()
    }

    fn get_type() -> AnalysedType {
        todo!()
    }
}

impl IntoValue for SerializableDateTime {
    fn into_value(self) -> Value {
        todo!()
    }

    fn get_type() -> AnalysedType {
        todo!()
    }
}

impl IntoValue for SerializableIpAddresses {
    fn into_value(self) -> Value {
        todo!()
    }

    fn get_type() -> AnalysedType {
        todo!()
    }
}

impl IntoValue for SerializableFileTimes {
    fn into_value(self) -> Value {
        todo!()
    }

    fn get_type() -> AnalysedType {
        todo!()
    }
}

impl IntoValue for WorkerId {
    fn into_value(self) -> Value {
        todo!()
    }

    fn get_type() -> AnalysedType {
        todo!()
    }
}

impl IntoValue for Uuid {
    fn into_value(self) -> Value {
        todo!()
    }

    fn get_type() -> AnalysedType {
        todo!()
    }
}
