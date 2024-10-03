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
    SerializableErrorCode, SerializableHttpRequest, SerializableResponse,
    SerializableResponseHeaders,
};
use crate::durable_host::serialized::{
    SerializableDateTime, SerializableError, SerializableFileTimes, SerializableIpAddresses,
    SerializableStreamError,
};
use crate::durable_host::wasm_rpc::serialized::{
    SerializableInvokeRequest, SerializableInvokeResult,
};
use crate::error::GolemError;
use crate::services::oplog::Oplog;
use crate::services::rpc::RpcError;
use crate::services::worker_proxy::WorkerProxyError;
use bincode::Decode;
use golem_api_grpc::proto::golem::worker::UpdateMode;
use golem_common::config::RetryConfig;
use golem_common::model::oplog::{
    IndexedResourceKey, LogLevel, OplogEntry, OplogIndex, UpdateDescription, WorkerError,
    WorkerResourceId, WrappedFunctionType,
};
use golem_common::model::regions::OplogRegion;
use golem_common::model::{
    AccountId, ComponentVersion, IdempotencyKey, PromiseId, Timestamp, WorkerId, WorkerInvocation,
};
use golem_common::serialization::try_deserialize;
use golem_wasm_ast::analysis::analysed_type::{
    case, field, list, option, record, result, str, u64, unit_case, variant,
};
use golem_wasm_ast::analysis::{AnalysedType, NameOptionTypePair, TypeVariant};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::{IntoValue, Value, ValueAndType};
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
        request: ValueAndType,
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
            OplogEntry::ImportedFunctionInvokedV1 {
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
                    request: Self::no_request_payload()?,
                    response: value,
                    wrapped_function_type,
                })
            }
            OplogEntry::ImportedFunctionInvoked {
                timestamp,
                function_name,
                request,
                response,
                wrapped_function_type,
            } => {
                let request_bytes = oplog.download_payload(&request).await?;
                let response_bytes = oplog.download_payload(&response).await?;
                let request =
                    Self::encode_host_function_request_as_value(&function_name, &request_bytes)?;
                let response =
                    Self::encode_host_function_response_as_value(&function_name, &response_bytes)?;
                Ok(PublicOplogEntry::ImportedFunctionInvoked {
                    timestamp,
                    function_name,
                    request,
                    response,
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

    fn no_request_payload() -> Result<ValueAndType, String> {
        Ok(ValueAndType::new(Value::Option(None), option(str())))
    }

    fn encode_host_function_request_as_value(
        function_name: &str,
        bytes: &[u8],
    ) -> Result<ValueAndType, String> {
        match function_name {
            "golem::rpc::future-invoke-result::get" => {
                let payload: SerializableInvokeRequest = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "http::types::future_incoming_response::get" => {
                let payload: SerializableHttpRequest = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem io::poll::poll" => Self::no_request_payload(),
            "golem blobstore::container::object_info" => {
                let payload: (String, String) = Self::try_deserialize(bytes)?;
                Ok(container_and_object(payload.0, payload.1))
            }
            "golem blobstore::container::delete_objects" => {
                let payload: (String, Vec<String>) = Self::try_deserialize(bytes)?;
                Ok(container_and_objects(payload.0, payload.1))
            }
            "golem blobstore::container::list_objects" => {
                let payload: String = Self::try_deserialize(bytes)?;
                Ok(container(payload))
            }
            "golem blobstore::container::get_data" => {
                let payload: (String, String, u64, u64) = Self::try_deserialize(bytes)?;
                Ok(ValueAndType::new(
                    Value::Record(vec![
                        Value::String(payload.0),
                        Value::String(payload.1),
                        Value::U64(payload.2),
                        Value::U64(payload.3),
                    ]),
                    record(vec![
                        field("container", str()),
                        field("object", str()),
                        field("begin", u64()),
                        field("end", u64()),
                    ]),
                ))
            }
            "golem blobstore::container::write_data" => {
                let payload: (String, String, u64) = Self::try_deserialize(bytes)?;
                Ok(ValueAndType::new(
                    Value::Record(vec![
                        Value::String(payload.0),
                        Value::String(payload.1),
                        Value::U64(payload.2),
                    ]),
                    record(vec![
                        field("container", str()),
                        field("object", str()),
                        field("length", u64()),
                    ]),
                ))
            }
            "golem blobstore::container::delete_object" => {
                let payload: (String, String) = Self::try_deserialize(bytes)?;
                Ok(container_and_object(payload.0, payload.1))
            }
            "golem blobstore::container::has_object" => {
                let payload: (String, String) = Self::try_deserialize(bytes)?;
                Ok(container_and_object(payload.0, payload.1))
            }
            "golem blobstore::container::clear" => {
                let payload: String = Self::try_deserialize(bytes)?;
                Ok(container(payload))
            }
            "golem blobstore::blobstore::copy_object" => {
                let payload: (String, String, String, String) = Self::try_deserialize(bytes)?;
                Ok(ValueAndType::new(
                    Value::Record(vec![
                        Value::String(payload.0),
                        Value::String(payload.1),
                        Value::String(payload.2),
                        Value::String(payload.3),
                    ]),
                    record(vec![
                        field("src_container", str()),
                        field("src_object", str()),
                        field("dest_container", str()),
                        field("dest_object", str()),
                    ]),
                ))
            }
            "golem blobstore::blobstore::delete_container" => {
                let payload: String = Self::try_deserialize(bytes)?;
                Ok(container(payload))
            }
            "golem blobstore::blobstore::create_container" => {
                let payload: String = Self::try_deserialize(bytes)?;
                Ok(container(payload))
            }
            "golem blobstore::blobstore::get_container" => {
                let payload: String = Self::try_deserialize(bytes)?;
                Ok(container(payload))
            }
            "golem blobstore::blobstore::container_exists" => {
                let payload: String = Self::try_deserialize(bytes)?;
                Ok(container(payload))
            }
            "golem blobstore::blobstore::move_object" => {
                let payload: (String, String, String, String) = Self::try_deserialize(bytes)?;
                Ok(ValueAndType::new(
                    Value::Record(vec![
                        Value::String(payload.0),
                        Value::String(payload.1),
                        Value::String(payload.2),
                        Value::String(payload.3),
                    ]),
                    record(vec![
                        field("src_container", str()),
                        field("src_object", str()),
                        field("dest_container", str()),
                        field("dest_object", str()),
                    ]),
                ))
            }
            "golem_environment::get_arguments" => Self::no_request_payload(),
            "golem_environment::get_environment" => Self::no_request_payload(),
            "golem_environment::initial_cwd" => Self::no_request_payload(),
            "monotonic_clock::resolution" => Self::no_request_payload(),
            "monotonic_clock::now" => Self::no_request_payload(),
            "monotonic_clock::subscribe_duration" => Self::no_request_payload(),
            "wall_clock::now" => Self::no_request_payload(),
            "wall_clock::resolution" => Self::no_request_payload(),
            "golem_delete_promise" => {
                let payload: PromiseId = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem_complete_promise" => {
                let payload: PromiseId = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem::api::update-worker" => {
                let payload: (WorkerId, ComponentVersion, UpdateMode) =
                    Self::try_deserialize(bytes)?;
                Ok(ValueAndType::new(
                    Value::Record(vec![
                        payload.0.into_value(),
                        payload.1.into_value(),
                        Value::String(format!("{:?}", payload.2)),
                    ]),
                    record(vec![
                        field("worker_id", WorkerId::get_type()),
                        field("component_version", u64()),
                        field("update_mode", str()),
                    ]),
                ))
            }
            "http::types::incoming_body_stream::skip" => {
                let payload: SerializableHttpRequest = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "http::types::incoming_body_stream::read" => {
                let payload: SerializableHttpRequest = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "http::types::incoming_body_stream::blocking_read" => {
                let payload: SerializableHttpRequest = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "http::types::incoming_body_stream::blocking_skip" => {
                let payload: SerializableHttpRequest = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem keyvalue::eventual::delete" => {
                let payload: (String, String) = Self::try_deserialize(bytes)?;
                Ok(bucket_and_key(payload.0, payload.1))
            }
            "golem keyvalue::eventual::get" => {
                let payload: (String, String) = Self::try_deserialize(bytes)?;
                Ok(bucket_and_key(payload.0, payload.1))
            }
            "golem keyvalue::eventual::set" => {
                let payload: (String, String, u64) = Self::try_deserialize(bytes)?;
                Ok(ValueAndType::new(
                    Value::Record(vec![
                        Value::String(payload.0),
                        Value::String(payload.1),
                        Value::U64(payload.2),
                    ]),
                    record(vec![
                        field("bucket", str()),
                        field("key", str()),
                        field("value", u64()),
                    ]),
                ))
            }
            "golem keyvalue::eventual::exists" => {
                let payload: (String, String) = Self::try_deserialize(bytes)?;
                Ok(bucket_and_key(payload.0, payload.1))
            }
            "golem keyvalue::eventual_batch::set_many" => {
                let payload: (String, Vec<(String, u64)>) = Self::try_deserialize(bytes)?;
                Ok(ValueAndType::new(
                    Value::Record(vec![
                        Value::String(payload.0),
                        Value::List(
                            payload
                                .1
                                .into_iter()
                                .map(|(key, value)| {
                                    Value::Record(vec![Value::String(key), Value::U64(value)])
                                })
                                .collect(),
                        ),
                    ]),
                    record(vec![
                        field("bucket", str()),
                        field(
                            "key_values",
                            list(record(vec![field("key", str()), field("length", u64())])),
                        ),
                    ]),
                ))
            }
            "golem keyvalue::eventual_batch::get_many" => {
                let payload: (String, Vec<String>) = Self::try_deserialize(bytes)?;
                Ok(bucket_and_keys(payload.0, payload.1))
            }
            "golem keyvalue::eventual_batch::get_keys" => {
                let payload: String = Self::try_deserialize(bytes)?;
                Ok(bucket(payload))
            }
            "golem keyvalue::eventual_batch::delete_many" => {
                let payload: (String, Vec<String>) = Self::try_deserialize(bytes)?;
                Ok(bucket_and_keys(payload.0, payload.1))
            }
            "golem random::insecure::get_insecure_random_bytes" => Self::no_request_payload(),
            "golem random::insecure::get_insecure_random_u64" => Self::no_request_payload(),
            "golem random::insecure_seed::insecure_seed" => Self::no_request_payload(),
            "golem random::get_random_bytes" => Self::no_request_payload(),
            "golem random::get_random_u64" => Self::no_request_payload(),
            "sockets::ip_name_lookup::resolve_addresses" => {
                let payload: String = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem::rpc::wasm-rpc::invoke" => {
                let payload: SerializableInvokeRequest = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem::rpc::wasm-rpc::invoke-and-await" => {
                let payload: SerializableInvokeRequest = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem::rpc::wasm-rpc::generate_unique_local_worker_id" => Self::no_request_payload(),
            "cli::preopens::get_directories" => {
                let payload: Result<Vec<String>, SerializableError> = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "filesystem::types::descriptor::stat" => {
                let payload: String = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "filesystem::types::descriptor::stat_at" => {
                let payload: String = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem api::generate_idempotency_key" => Self::no_request_payload(),
            "golem http::types::future_trailers::get" => {
                let payload: SerializableHttpRequest = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem::rpc::wasm-rpc::invoke idempotency key" => Self::no_request_payload(),
            "golem::rpc::wasm-rpc::invoke-and-await idempotency key" => Self::no_request_payload(),
            "golem::rpc::wasm-rpc::async-invoke-and-await idempotency key" => {
                Self::no_request_payload()
            }
            _ => Err(format!("Unsupported host function name: {}", function_name)),
        }
    }

    fn encode_host_function_response_as_value(
        function_name: &str,
        bytes: &[u8],
    ) -> Result<ValueAndType, String> {
        match function_name {
            "golem::rpc::future-invoke-result::get" => {
                let payload: SerializableInvokeResult = Self::try_deserialize(bytes)?;
                match payload {
                    SerializableInvokeResult::Failed(error) => Ok(ValueAndType::new(
                        Value::Variant {
                            case_idx: 0,
                            case_value: Some(Box::new(error.into_value())),
                        },
                        variant(vec![
                            case("Failed", SerializableError::get_type()),
                            unit_case("Pending"),
                            unit_case("Completed"),
                        ]),
                    )),
                    SerializableInvokeResult::Pending => Ok(ValueAndType::new(
                        Value::Variant {
                            case_idx: 1,
                            case_value: None,
                        },
                        variant(vec![
                            case("Failed", SerializableError::get_type()),
                            unit_case("Pending"),
                            unit_case("Completed"),
                        ]),
                    )),
                    SerializableInvokeResult::Completed(Ok(value)) => {
                        let typ: AnalysedType = (&value).try_into()?;
                        let value: Value = value.try_into()?;
                        Ok(ValueAndType::new(
                            Value::Variant {
                                case_idx: 2,
                                case_value: Some(Box::new(Value::Result(Ok(Some(Box::new(
                                    value,
                                )))))),
                            },
                            variant(vec![
                                case("Failed", SerializableError::get_type()),
                                unit_case("Pending"),
                                case("Completed", result(typ, RpcError::get_type())),
                            ]),
                        ))
                    }
                    SerializableInvokeResult::Completed(Err(rpc_error)) => Ok(ValueAndType::new(
                        Value::Variant {
                            case_idx: 2,
                            case_value: Some(Box::new(Value::Result(Err(Some(Box::new(
                                rpc_error.into_value(),
                            )))))),
                        },
                        variant(vec![
                            case("Failed", SerializableError::get_type()),
                            unit_case("Pending"),
                            case("Completed", result(record(vec![]), RpcError::get_type())),
                        ]),
                    )),
                }
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
                let payload: Result<TypeAnnotatedValue, SerializableError> =
                    Self::try_deserialize(bytes)?;
                // TODO: must prepare for that it's not TypeAnnotatedValue but WitValue (old versions)
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

impl IntoValue for SerializableInvokeRequest {
    fn into_value(self) -> Value {
        todo!()
    }

    fn get_type() -> AnalysedType {
        todo!()
    }
}

impl IntoValue for SerializableHttpRequest {
    fn into_value(self) -> Value {
        todo!()
    }

    fn get_type() -> AnalysedType {
        todo!()
    }
}

fn container(container: String) -> ValueAndType {
    ValueAndType::new(
        Value::Record(vec![Value::String(container)]),
        record(vec![field("container", str())]),
    )
}

fn container_and_object(container: String, object: String) -> ValueAndType {
    ValueAndType::new(
        Value::Record(vec![Value::String(container), Value::String(object)]),
        record(vec![field("container", str()), field("object", str())]),
    )
}

fn container_and_objects(container: String, objects: Vec<String>) -> ValueAndType {
    ValueAndType::new(
        Value::Record(vec![
            Value::String(container),
            Value::List(objects.into_iter().map(Value::String).collect()),
        ]),
        record(vec![
            field("container", str()),
            field("objects", list(str())),
        ]),
    )
}

fn bucket(bucket: String) -> ValueAndType {
    ValueAndType::new(
        Value::Record(vec![Value::String(bucket)]),
        record(vec![field("bucket", str())]),
    )
}

fn bucket_and_key(bucket: String, key: String) -> ValueAndType {
    ValueAndType::new(
        Value::Record(vec![Value::String(bucket), Value::String(key)]),
        record(vec![field("bucket", str()), field("key", str())]),
    )
}

fn bucket_and_keys(bucket: String, keys: Vec<String>) -> ValueAndType {
    ValueAndType::new(
        Value::Record(vec![
            Value::String(bucket),
            Value::List(keys.into_iter().map(Value::String).collect()),
        ]),
        record(vec![field("bucket", str()), field("keys", list(str()))]),
    )
}
