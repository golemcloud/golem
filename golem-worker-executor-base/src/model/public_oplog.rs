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
    SerializableDnsErrorPayload, SerializableErrorCode, SerializableFieldSizePayload,
    SerializableHttpMethod, SerializableHttpRequest, SerializableResponse,
    SerializableResponseHeaders, SerializableTlsAlertReceivedPayload,
};
use crate::durable_host::serialized::{
    SerializableDateTime, SerializableError, SerializableFileTimes, SerializableIpAddress,
    SerializableIpAddresses, SerializableStreamError,
};
use crate::durable_host::wasm_rpc::serialized::{
    SerializableInvokeRequest, SerializableInvokeResult,
};
use crate::error::GolemError;
use crate::model::InterruptKind;
use crate::services::component::ComponentService;
use crate::services::oplog::Oplog;
use crate::services::rpc::RpcError;
use crate::services::worker_proxy::WorkerProxyError;
use bincode::Decode;
use golem_api_grpc::proto::golem::worker::UpdateMode;
use golem_common::config::RetryConfig;
use golem_common::model::exports::function_by_name;
use golem_common::model::oplog::{
    IndexedResourceKey, LogLevel, OplogEntry, OplogIndex, UpdateDescription, WorkerError,
    WorkerResourceId, WrappedFunctionType,
};
use golem_common::model::regions::OplogRegion;
use golem_common::model::{
    AccountId, ComponentId, ComponentVersion, IdempotencyKey, PromiseId, ShardId, Timestamp,
    WorkerId, WorkerInvocation,
};
use golem_common::serialization::try_deserialize;
use golem_wasm_ast::analysis::analysed_type::{
    case, field, list, option, r#enum, record, result, result_err, str, tuple, u16, u32, u64, u8,
    unit_case, variant,
};
use golem_wasm_ast::analysis::{AnalysedType, NameOptionTypePair, TypeVariant};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::{IntoValue, IntoValueAndType, Value, ValueAndType, WitValue};
use std::collections::HashMap;
use std::net::IpAddr;
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
    pub async fn from_oplog_entry(
        value: OplogEntry,
        oplog: Arc<dyn Oplog + Send + Sync>,
        components: Arc<dyn ComponentService + Send + Sync>,
        component_id: &ComponentId,
        component_version: ComponentVersion,
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
                    request: Self::no_payload()?,
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
                let params = proto_params
                    .into_iter()
                    .map(Value::try_from)
                    .collect::<Result<Vec<_>, _>>()?;

                let metadata = components
                    .get_metadata(component_id, Some(component_version))
                    .await
                    .map_err(|err| err.to_string())?; // TODO: decide if we want to propagate GolemError out of here
                let function = function_by_name(&metadata.exports, &function_name)?.ok_or(
                    format!("Exported function {function_name} not found in component {component_id} version {component_version}")
                )?;
                let request = function
                    .parameters
                    .iter()
                    .zip(params)
                    .map(|(param, value)| ValueAndType::new(value, param.typ.clone()))
                    .collect();

                Ok(PublicOplogEntry::ExportedFunctionInvoked {
                    timestamp,
                    function_name,
                    request,
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

    fn no_payload() -> Result<ValueAndType, String> {
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
            "golem io::poll::poll" => Self::no_payload(),
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
            "golem_environment::get_arguments" => Self::no_payload(),
            "golem_environment::get_environment" => Self::no_payload(),
            "golem_environment::initial_cwd" => Self::no_payload(),
            "monotonic_clock::resolution" => Self::no_payload(),
            "monotonic_clock::now" => Self::no_payload(),
            "monotonic_clock::subscribe_duration" => Self::no_payload(),
            "wall_clock::now" => Self::no_payload(),
            "wall_clock::resolution" => Self::no_payload(),
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
            "golem random::insecure::get_insecure_random_bytes" => Self::no_payload(),
            "golem random::insecure::get_insecure_random_u64" => Self::no_payload(),
            "golem random::insecure_seed::insecure_seed" => Self::no_payload(),
            "golem random::get_random_bytes" => Self::no_payload(),
            "golem random::get_random_u64" => Self::no_payload(),
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
            "golem::rpc::wasm-rpc::generate_unique_local_worker_id" => Self::no_payload(),
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
            "golem api::generate_idempotency_key" => Self::no_payload(),
            "golem http::types::future_trailers::get" => {
                let payload: SerializableHttpRequest = Self::try_deserialize(bytes)?;
                Ok(payload.into_value_and_type())
            }
            "golem::rpc::wasm-rpc::invoke idempotency key" => Self::no_payload(),
            "golem::rpc::wasm-rpc::invoke-and-await idempotency key" => Self::no_payload(),
            "golem::rpc::wasm-rpc::async-invoke-and-await idempotency key" => Self::no_payload(),
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
                let payload: Result<Result<TypeAnnotatedValue, SerializableError>, String> =
                    Self::try_deserialize(bytes);

                match payload {
                    Err(_) => {
                        let _payload: Result<WitValue, SerializableError> =
                            Self::try_deserialize(bytes)?;
                        Self::no_payload()
                    }
                    Ok(Ok(payload)) => {
                        let typ: AnalysedType = (&payload).try_into()?;
                        let value: Value = payload.try_into()?;
                        Ok(ValueAndType::new(
                            Value::Result(Ok(Some(Box::new(value)))),
                            result(typ, SerializableError::get_type()),
                        ))
                    }
                    Ok(Err(error)) => Ok(ValueAndType::new(
                        Value::Result(Err(Some(Box::new(error.into_value())))),
                        result_err(SerializableError::get_type()),
                    )),
                }
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
        match self {
            SerializableStreamError::Closed => Value::Variant {
                case_idx: 0,
                case_value: None,
            },
            SerializableStreamError::LastOperationFailed(error) => Value::Variant {
                case_idx: 1,
                case_value: Some(Box::new(error.into_value())),
            },
            SerializableStreamError::Trap(error) => Value::Variant {
                case_idx: 2,
                case_value: Some(Box::new(error.into_value())),
            },
        }
    }

    fn get_type() -> AnalysedType {
        AnalysedType::Variant(TypeVariant {
            cases: vec![
                NameOptionTypePair {
                    name: "Closed".to_string(),
                    typ: None,
                },
                NameOptionTypePair {
                    name: "LastOperationFailed".to_string(),
                    typ: Some(SerializableError::get_type()),
                },
                NameOptionTypePair {
                    name: "Trap".to_string(),
                    typ: Some(SerializableError::get_type()),
                },
            ],
        })
    }
}

impl IntoValue for RpcError {
    fn into_value(self) -> Value {
        match self {
            RpcError::ProtocolError { details } => Value::Variant {
                case_idx: 0,
                case_value: Some(Box::new(details.into_value())),
            },
            RpcError::Denied { details } => Value::Variant {
                case_idx: 1,
                case_value: Some(Box::new(details.into_value())),
            },
            RpcError::NotFound { details } => Value::Variant {
                case_idx: 2,
                case_value: Some(Box::new(details.into_value())),
            },
            RpcError::RemoteInternalError { details } => Value::Variant {
                case_idx: 3,
                case_value: Some(Box::new(details.into_value())),
            },
        }
    }

    fn get_type() -> AnalysedType {
        AnalysedType::Variant(TypeVariant {
            cases: vec![
                NameOptionTypePair {
                    name: "ProtocolError".to_string(),
                    typ: Some(String::get_type()),
                },
                NameOptionTypePair {
                    name: "Denied".to_string(),
                    typ: Some(String::get_type()),
                },
                NameOptionTypePair {
                    name: "NotFound".to_string(),
                    typ: Some(String::get_type()),
                },
                NameOptionTypePair {
                    name: "RemoteInternalError".to_string(),
                    typ: Some(String::get_type()),
                },
            ],
        })
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
        let SerializableResponseHeaders { status, headers } = self;
        let headers: HashMap<String, String> = headers
            .into_iter()
            .map(|(k, v)| (k, String::from_utf8_lossy(&v).to_string()))
            .collect();
        Value::Record(vec![status.into_value(), headers.into_value()])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("status", u16()),
            field("headers", HashMap::<String, String>::get_type()),
        ])
    }
}

impl IntoValue for SerializableErrorCode {
    fn into_value(self) -> Value {
        match self {
            SerializableErrorCode::DnsTimeout => Value::Variant {
                case_idx: 0,
                case_value: None,
            },
            SerializableErrorCode::DnsError(payload) => Value::Variant {
                case_idx: 1,
                case_value: Some(Box::new(payload.into_value())),
            },
            SerializableErrorCode::DestinationNotFound => Value::Variant {
                case_idx: 2,
                case_value: None,
            },
            SerializableErrorCode::DestinationUnavailable => Value::Variant {
                case_idx: 3,
                case_value: None,
            },
            SerializableErrorCode::DestinationIpProhibited => Value::Variant {
                case_idx: 4,
                case_value: None,
            },
            SerializableErrorCode::DestinationIpUnroutable => Value::Variant {
                case_idx: 5,
                case_value: None,
            },
            SerializableErrorCode::ConnectionRefused => Value::Variant {
                case_idx: 6,
                case_value: None,
            },
            SerializableErrorCode::ConnectionTerminated => Value::Variant {
                case_idx: 7,
                case_value: None,
            },
            SerializableErrorCode::ConnectionTimeout => Value::Variant {
                case_idx: 8,
                case_value: None,
            },
            SerializableErrorCode::ConnectionReadTimeout => Value::Variant {
                case_idx: 9,
                case_value: None,
            },
            SerializableErrorCode::ConnectionWriteTimeout => Value::Variant {
                case_idx: 10,
                case_value: None,
            },
            SerializableErrorCode::ConnectionLimitReached => Value::Variant {
                case_idx: 11,
                case_value: None,
            },
            SerializableErrorCode::TlsProtocolError => Value::Variant {
                case_idx: 12,
                case_value: None,
            },
            SerializableErrorCode::TlsCertificateError => Value::Variant {
                case_idx: 13,
                case_value: None,
            },
            SerializableErrorCode::TlsAlertReceived(payload) => Value::Variant {
                case_idx: 14,
                case_value: Some(Box::new(payload.into_value())),
            },
            SerializableErrorCode::HttpRequestDenied => Value::Variant {
                case_idx: 15,
                case_value: None,
            },
            SerializableErrorCode::HttpRequestLengthRequired => Value::Variant {
                case_idx: 16,
                case_value: None,
            },
            SerializableErrorCode::HttpRequestBodySize(payload) => Value::Variant {
                case_idx: 17,
                case_value: Some(Box::new(payload.into_value())),
            },
            SerializableErrorCode::HttpRequestMethodInvalid => Value::Variant {
                case_idx: 18,
                case_value: None,
            },
            SerializableErrorCode::HttpRequestUriInvalid => Value::Variant {
                case_idx: 19,
                case_value: None,
            },
            SerializableErrorCode::HttpRequestUriTooLong => Value::Variant {
                case_idx: 20,
                case_value: None,
            },
            SerializableErrorCode::HttpRequestHeaderSectionSize(payload) => Value::Variant {
                case_idx: 21,
                case_value: Some(Box::new(payload.into_value())),
            },
            SerializableErrorCode::HttpRequestHeaderSize(payload) => Value::Variant {
                case_idx: 22,
                case_value: Some(Box::new(payload.into_value())),
            },
            SerializableErrorCode::HttpRequestTrailerSectionSize(payload) => Value::Variant {
                case_idx: 23,
                case_value: Some(Box::new(payload.into_value())),
            },
            SerializableErrorCode::HttpRequestTrailerSize(payload) => Value::Variant {
                case_idx: 24,
                case_value: Some(Box::new(payload.into_value())),
            },
            SerializableErrorCode::HttpResponseIncomplete => Value::Variant {
                case_idx: 25,
                case_value: None,
            },
            SerializableErrorCode::HttpResponseHeaderSectionSize(payload) => Value::Variant {
                case_idx: 26,
                case_value: Some(Box::new(payload.into_value())),
            },
            SerializableErrorCode::HttpResponseHeaderSize(payload) => Value::Variant {
                case_idx: 27,
                case_value: Some(Box::new(payload.into_value())),
            },
            SerializableErrorCode::HttpResponseBodySize(payload) => Value::Variant {
                case_idx: 28,
                case_value: Some(Box::new(payload.into_value())),
            },
            SerializableErrorCode::HttpResponseTrailerSectionSize(payload) => Value::Variant {
                case_idx: 29,
                case_value: Some(Box::new(payload.into_value())),
            },
            SerializableErrorCode::HttpResponseTrailerSize(payload) => Value::Variant {
                case_idx: 30,
                case_value: Some(Box::new(payload.into_value())),
            },
            SerializableErrorCode::HttpResponseTransferCoding(payload) => Value::Variant {
                case_idx: 31,
                case_value: Some(Box::new(payload.into_value())),
            },
            SerializableErrorCode::HttpResponseContentCoding(payload) => Value::Variant {
                case_idx: 32,
                case_value: Some(Box::new(payload.into_value())),
            },
            SerializableErrorCode::HttpResponseTimeout => Value::Variant {
                case_idx: 33,
                case_value: None,
            },
            SerializableErrorCode::HttpUpgradeFailed => Value::Variant {
                case_idx: 34,
                case_value: None,
            },
            SerializableErrorCode::HttpProtocolError => Value::Variant {
                case_idx: 35,
                case_value: None,
            },
            SerializableErrorCode::LoopDetected => Value::Variant {
                case_idx: 36,
                case_value: None,
            },
            SerializableErrorCode::ConfigurationError => Value::Variant {
                case_idx: 37,
                case_value: None,
            },
            SerializableErrorCode::InternalError(payload) => Value::Variant {
                case_idx: 38,
                case_value: Some(Box::new(payload.into_value())),
            },
        }
    }

    fn get_type() -> AnalysedType {
        variant(vec![
            unit_case("DnsTimeout"),
            case("DnsError", SerializableDnsErrorPayload::get_type()),
            unit_case("DestinationNotFound"),
            unit_case("DestinationUnavailable"),
            unit_case("DestinationIpProhibited"),
            unit_case("DestinationIpUnroutable"),
            unit_case("ConnectionRefused"),
            unit_case("ConnectionTerminated"),
            unit_case("ConnectionTimeout"),
            unit_case("ConnectionReadTimeout"),
            unit_case("ConnectionWriteTimeout"),
            unit_case("ConnectionLimitReached"),
            unit_case("TlsProtocolError"),
            unit_case("TlsCertificateError"),
            case(
                "TlsAlertReceived",
                SerializableTlsAlertReceivedPayload::get_type(),
            ),
            unit_case("HttpRequestDenied"),
            unit_case("HttpRequestLengthRequired"),
            case("HttpRequestBodySize", option(u64())),
            unit_case("HttpRequestMethodInvalid"),
            unit_case("HttpRequestUriInvalid"),
            unit_case("HttpRequestUriTooLong"),
            case("HttpRequestHeaderSectionSize", option(u32())),
            case(
                "HttpRequestHeaderSize",
                option(SerializableFieldSizePayload::get_type()),
            ),
            case("HttpRequestTrailerSectionSize", option(u32())),
            case(
                "HttpRequestTrailerSize",
                SerializableFieldSizePayload::get_type(),
            ),
            unit_case("HttpResponseIncomplete"),
            case("HttpResponseHeaderSectionSize", option(u32())),
            case(
                "HttpResponseHeaderSize",
                SerializableFieldSizePayload::get_type(),
            ),
            case("HttpResponseBodySize", option(u64())),
            case("HttpResponseTrailerSectionSize", option(u32())),
            case(
                "HttpResponseTrailerSize",
                SerializableFieldSizePayload::get_type(),
            ),
            case("HttpResponseTransferCoding", option(str())),
            case("HttpResponseContentCoding", option(str())),
            unit_case("HttpResponseTimeout"),
            unit_case("HttpUpgradeFailed"),
            unit_case("HttpProtocolError"),
            unit_case("LoopDetected"),
            unit_case("ConfigurationError"),
            case("InternalError", option(str())),
        ])
    }
}

impl IntoValue for SerializableDnsErrorPayload {
    fn into_value(self) -> Value {
        Value::Record(vec![self.rcode.into_value(), self.info_code.into_value()])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("rcode", option(str())),
            field("info_code", option(u16())),
        ])
    }
}

impl IntoValue for SerializableTlsAlertReceivedPayload {
    fn into_value(self) -> Value {
        Value::Record(vec![
            self.alert_id.into_value(),
            self.alert_message.into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("alert_id", option(u8())),
            field("alert_message", option(str())),
        ])
    }
}

impl IntoValue for SerializableFieldSizePayload {
    fn into_value(self) -> Value {
        Value::Record(vec![
            self.field_name.into_value(),
            self.field_size.into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("field_name", option(str())),
            field("field_size", option(u32())),
        ])
    }
}

impl IntoValue for GolemError {
    fn into_value(self) -> Value {
        fn into_value(value: GolemError, top_level: bool) -> Value {
            match value {
                GolemError::InvalidRequest { details } => Value::Variant {
                    case_idx: 0,
                    case_value: Some(Box::new(Value::Record(vec![details.into_value()]))),
                },
                GolemError::WorkerAlreadyExists { worker_id } => Value::Variant {
                    case_idx: 1,
                    case_value: Some(Box::new(Value::Record(vec![worker_id.into_value()]))),
                },
                GolemError::WorkerNotFound { worker_id } => Value::Variant {
                    case_idx: 2,
                    case_value: Some(Box::new(Value::Record(vec![worker_id.into_value()]))),
                },
                GolemError::WorkerCreationFailed { worker_id, details } => Value::Variant {
                    case_idx: 3,
                    case_value: Some(Box::new(Value::Record(vec![
                        worker_id.into_value(),
                        details.into_value(),
                    ]))),
                },
                GolemError::FailedToResumeWorker { worker_id, reason } => Value::Variant {
                    case_idx: 4,
                    case_value: Some(Box::new(Value::Record(vec![
                        worker_id.into_value(),
                        if top_level {
                            into_value(*reason, false)
                        } else {
                            reason.to_string().into_value()
                        },
                    ]))),
                },
                GolemError::ComponentDownloadFailed {
                    component_id,
                    component_version,
                    reason,
                } => Value::Variant {
                    case_idx: 5,
                    case_value: Some(Box::new(Value::Record(vec![
                        component_id.into_value(),
                        component_version.into_value(),
                        reason.into_value(),
                    ]))),
                },
                GolemError::ComponentParseFailed {
                    component_id,
                    component_version,
                    reason,
                } => Value::Variant {
                    case_idx: 6,
                    case_value: Some(Box::new(Value::Record(vec![
                        component_id.into_value(),
                        component_version.into_value(),
                        reason.into_value(),
                    ]))),
                },
                GolemError::GetLatestVersionOfComponentFailed {
                    component_id,
                    reason,
                } => Value::Variant {
                    case_idx: 7,
                    case_value: Some(Box::new(Value::Record(vec![
                        component_id.into_value(),
                        reason.into_value(),
                    ]))),
                },
                GolemError::PromiseNotFound { promise_id } => Value::Variant {
                    case_idx: 8,
                    case_value: Some(Box::new(Value::Record(vec![promise_id.into_value()]))),
                },
                GolemError::PromiseDropped { promise_id } => Value::Variant {
                    case_idx: 9,
                    case_value: Some(Box::new(Value::Record(vec![promise_id.into_value()]))),
                },
                GolemError::PromiseAlreadyCompleted { promise_id } => Value::Variant {
                    case_idx: 10,
                    case_value: Some(Box::new(Value::Record(vec![promise_id.into_value()]))),
                },
                GolemError::Interrupted { kind } => Value::Variant {
                    case_idx: 11,
                    case_value: Some(Box::new(Value::Record(vec![kind.into_value()]))),
                },
                GolemError::ParamTypeMismatch { details } => Value::Variant {
                    case_idx: 12,
                    case_value: Some(Box::new(Value::Record(vec![details.into_value()]))),
                },
                GolemError::NoValueInMessage => Value::Variant {
                    case_idx: 13,
                    case_value: None,
                },
                GolemError::ValueMismatch { details } => Value::Variant {
                    case_idx: 14,
                    case_value: Some(Box::new(Value::Record(vec![details.into_value()]))),
                },
                GolemError::UnexpectedOplogEntry { expected, got } => Value::Variant {
                    case_idx: 15,
                    case_value: Some(Box::new(Value::Record(vec![
                        expected.into_value(),
                        got.into_value(),
                    ]))),
                },
                GolemError::Runtime { details } => Value::Variant {
                    case_idx: 16,
                    case_value: Some(Box::new(Value::Record(vec![details.into_value()]))),
                },
                GolemError::InvalidShardId {
                    shard_id,
                    shard_ids,
                } => Value::Variant {
                    case_idx: 17,
                    case_value: Some(Box::new(Value::Record(vec![
                        shard_id.into_value(),
                        shard_ids.into_value(),
                    ]))),
                },
                GolemError::InvalidAccount => Value::Variant {
                    case_idx: 18,
                    case_value: None,
                },
                GolemError::PreviousInvocationFailed { details } => Value::Variant {
                    case_idx: 19,
                    case_value: Some(Box::new(Value::Record(vec![details.into_value()]))),
                },
                GolemError::PreviousInvocationExited => Value::Variant {
                    case_idx: 20,
                    case_value: None,
                },
                GolemError::Unknown { details } => Value::Variant {
                    case_idx: 21,
                    case_value: Some(Box::new(Value::Record(vec![details.into_value()]))),
                },
                GolemError::ShardingNotReady => Value::Variant {
                    case_idx: 22,
                    case_value: None,
                },
            }
        }
        into_value(self, true)
    }

    fn get_type() -> AnalysedType {
        fn get_type(top_level: bool) -> AnalysedType {
            variant(vec![
                case("InvalidRequest", record(vec![field("details", str())])),
                case(
                    "WorkerAlreadyExists",
                    record(vec![field("worker_id", WorkerId::get_type())]),
                ),
                case(
                    "WorkerNotFound",
                    record(vec![field("worker_id", WorkerId::get_type())]),
                ),
                case(
                    "WorkerCreationFailed",
                    record(vec![
                        field("worker_id", WorkerId::get_type()),
                        field("details", str()),
                    ]),
                ),
                case(
                    "FailedToResumeWorker",
                    record(vec![
                        field("worker_id", WorkerId::get_type()),
                        field(
                            "reason",
                            if top_level {
                                GolemError::get_type()
                            } else {
                                str()
                            },
                        ),
                    ]),
                ),
                case(
                    "ComponentDownloadFailed",
                    record(vec![
                        field("component_id", ComponentId::get_type()),
                        field("component_version", u64()),
                        field("reason", str()),
                    ]),
                ),
                case(
                    "ComponentParseFailed",
                    record(vec![
                        field("component_id", ComponentId::get_type()),
                        field("component_version", u64()),
                        field("reason", str()),
                    ]),
                ),
                case(
                    "GetLatestVersionOfComponentFailed",
                    record(vec![
                        field("component_id", ComponentId::get_type()),
                        field("reason", str()),
                    ]),
                ),
                case(
                    "PromiseNotFound",
                    record(vec![field("promise_id", PromiseId::get_type())]),
                ),
                case(
                    "PromiseDropped",
                    record(vec![field("promise_id", PromiseId::get_type())]),
                ),
                case(
                    "PromiseAlreadyCompleted",
                    record(vec![field("promise_id", PromiseId::get_type())]),
                ),
                case(
                    "Interrupted",
                    record(vec![field("kind", InterruptKind::get_type())]),
                ),
                case("ParamTypeMismatch", record(vec![field("details", str())])),
                unit_case("NoValueInMessage"),
                case("ValueMismatch", record(vec![field("details", str())])),
                case(
                    "UnexpectedOplogEntry",
                    record(vec![field("expected", str()), field("got", str())]),
                ),
                case("Runtime", record(vec![field("details", str())])),
                case(
                    "InvalidShardId",
                    record(vec![
                        field("shard_id", ShardId::get_type()),
                        field("shard_ids", list(ShardId::get_type())),
                    ]),
                ),
                unit_case("InvalidAccount"),
                case(
                    "PreviousInvocationFailed",
                    record(vec![field("details", str())]),
                ),
                unit_case("PreviousInvocationExited"),
                case("Unknown", record(vec![field("details", str())])),
                unit_case("ShardingNotReady"),
            ])
        }
        get_type(true)
    }
}

impl IntoValue for InterruptKind {
    fn into_value(self) -> Value {
        match self {
            InterruptKind::Interrupt => Value::Enum(0),
            InterruptKind::Restart => Value::Enum(1),
            InterruptKind::Suspend => Value::Enum(2),
            InterruptKind::Jump => Value::Enum(3),
        }
    }

    fn get_type() -> AnalysedType {
        r#enum(&["Interrupt", "Restart", "Suspend", "Jump"])
    }
}

impl IntoValue for WorkerProxyError {
    fn into_value(self) -> Value {
        match self {
            WorkerProxyError::BadRequest(errors) => Value::Variant {
                case_idx: 0,
                case_value: Some(Box::new(errors.into_value())),
            },
            WorkerProxyError::Unauthorized(error) => Value::Variant {
                case_idx: 1,
                case_value: Some(Box::new(error.into_value())),
            },
            WorkerProxyError::LimitExceeded(error) => Value::Variant {
                case_idx: 2,
                case_value: Some(Box::new(error.into_value())),
            },
            WorkerProxyError::NotFound(error) => Value::Variant {
                case_idx: 3,
                case_value: Some(Box::new(error.into_value())),
            },
            WorkerProxyError::AlreadyExists(error) => Value::Variant {
                case_idx: 4,
                case_value: Some(Box::new(error.into_value())),
            },
            WorkerProxyError::InternalError(error) => Value::Variant {
                case_idx: 5,
                case_value: Some(Box::new(error.into_value())),
            },
        }
    }

    fn get_type() -> AnalysedType {
        variant(vec![
            case("BadRequest", list(str())),
            case("Unauthorized", str()),
            case("LimitExceeded", str()),
            case("NotFound", str()),
            case("AlreadyExists", str()),
            case("InternalError", GolemError::get_type()),
        ])
    }
}

impl IntoValue for crate::services::blob_store::ObjectMetadata {
    fn into_value(self) -> Value {
        Value::Record(vec![
            Value::String(self.name),
            Value::String(self.container),
            Value::U64(self.created_at),
            Value::U64(self.size),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("name", str()),
            field("container", str()),
            field("created_at", u64()),
            field("size", u64()),
        ])
    }
}

impl IntoValue for SerializableDateTime {
    fn into_value(self) -> Value {
        Value::Record(vec![Value::U64(self.seconds), Value::U32(self.nanoseconds)])
    }

    fn get_type() -> AnalysedType {
        record(vec![field("seconds", u64()), field("nanoseconds", u32())])
    }
}

impl IntoValue for SerializableIpAddresses {
    fn into_value(self) -> Value {
        Value::List(self.0.into_iter().map(|v| v.into_value()).collect())
    }

    fn get_type() -> AnalysedType {
        list(SerializableIpAddress::get_type())
    }
}

impl IntoValue for SerializableIpAddress {
    fn into_value(self) -> Value {
        let addr = match self {
            SerializableIpAddress::IPv4 { address } => IpAddr::V4(address.into()),
            SerializableIpAddress::IPv6 { address } => IpAddr::V6(address.into()),
        };
        Value::String(addr.to_string())
    }

    fn get_type() -> AnalysedType {
        str()
    }
}

impl IntoValue for SerializableFileTimes {
    fn into_value(self) -> Value {
        Value::Record(vec![
            self.data_access_timestamp.into_value(),
            self.data_modification_timestamp.into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field(
                "data_access_timestamp",
                option(SerializableDateTime::get_type()),
            ),
            field(
                "data_modification_timestamp",
                option(SerializableDateTime::get_type()),
            ),
        ])
    }
}

impl IntoValue for SerializableHttpRequest {
    fn into_value(self) -> Value {
        Value::Record(vec![
            Value::String(self.uri),
            self.method.into_value(),
            self.headers.into_value(),
        ])
    }

    fn get_type() -> AnalysedType {
        record(vec![
            field("uri", str()),
            field("method", SerializableHttpMethod::get_type()),
            field("headers", HashMap::<String, String>::get_type()),
        ])
    }
}

impl IntoValue for SerializableHttpMethod {
    fn into_value(self) -> Value {
        match self {
            SerializableHttpMethod::Get => Value::Variant {
                case_idx: 0,
                case_value: None,
            },
            SerializableHttpMethod::Post => Value::Variant {
                case_idx: 1,
                case_value: None,
            },
            SerializableHttpMethod::Put => Value::Variant {
                case_idx: 2,
                case_value: None,
            },
            SerializableHttpMethod::Delete => Value::Variant {
                case_idx: 3,
                case_value: None,
            },
            SerializableHttpMethod::Head => Value::Variant {
                case_idx: 4,
                case_value: None,
            },
            SerializableHttpMethod::Connect => Value::Variant {
                case_idx: 5,
                case_value: None,
            },
            SerializableHttpMethod::Options => Value::Variant {
                case_idx: 6,
                case_value: None,
            },
            SerializableHttpMethod::Trace => Value::Variant {
                case_idx: 7,
                case_value: None,
            },
            SerializableHttpMethod::Patch => Value::Variant {
                case_idx: 8,
                case_value: None,
            },
            SerializableHttpMethod::Other(other) => Value::Variant {
                case_idx: 9,
                case_value: Some(Box::new(Value::String(other))),
            },
        }
    }

    fn get_type() -> AnalysedType {
        variant(vec![
            unit_case("Get"),
            unit_case("Post"),
            unit_case("Put"),
            unit_case("Delete"),
            unit_case("Head"),
            unit_case("Connect"),
            unit_case("Options"),
            unit_case("Trace"),
            unit_case("Patch"),
            case("Other", str()),
        ])
    }
}

impl IntoValueAndType for SerializableInvokeRequest {
    fn into_value_and_type(self) -> ValueAndType {
        ValueAndType::new(
            Value::Record(vec![
                self.remote_worker_id.into_value(),
                self.idempotency_key.into_value(),
                Value::String(self.function_name),
                Value::Tuple(
                    self.function_params
                        .iter()
                        .map(|v| v.value.clone())
                        .collect(),
                ),
            ]),
            record(vec![
                field("remote_worker_id", WorkerId::get_type()),
                field("idempotency_key", IdempotencyKey::get_type()),
                field("function_name", str()),
                field(
                    "function_params",
                    tuple(self.function_params.into_iter().map(|v| v.typ).collect()),
                ),
            ]),
        )
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