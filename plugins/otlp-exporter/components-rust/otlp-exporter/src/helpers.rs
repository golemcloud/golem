use golem_rust::bindings::golem::api::context::AttributeValue;
use golem_rust::bindings::golem::api::oplog::{Timestamp, WorkerError, WrappedFunctionType};
use golem_rust::golem_wasm::golem_core_1_5_x::types::{AgentId, ComponentId};
use golem_rust::wasip2::clocks::wall_clock::Datetime;
use std::collections::HashMap;

pub(crate) fn datetime_to_nanos(dt: &Datetime) -> u128 {
    (dt.seconds as u128) * 1_000_000_000 + (dt.nanoseconds as u128)
}

pub(crate) fn timestamp_to_nanos(ts: &Timestamp) -> u128 {
    datetime_to_nanos(&ts.timestamp)
}

pub(crate) fn worker_key(component_id: &ComponentId, worker_id: &AgentId) -> String {
    format!(
        "{:016x}{:016x}/{}",
        component_id.uuid.high_bits, component_id.uuid.low_bits, worker_id.agent_id
    )
}

pub(crate) fn format_uuid(high: u64, low: u64) -> String {
    uuid::Uuid::from_u64_pair(high, low).to_string()
}

pub(crate) fn attribute_value_to_string(v: &AttributeValue) -> String {
    match v {
        AttributeValue::String(s) => s.clone(),
    }
}

pub(crate) fn infer_span_kind(name: &str, attributes: &HashMap<String, String>) -> u32 {
    match name {
        "invoke-exported-function" => 2, // SERVER
        "rpc-connection" | "rpc-invocation" | "outgoing-http-request" => 3, // CLIENT
        _ => {
            if attributes.contains_key("request.method") && attributes.contains_key("request.uri") {
                2 // SERVER — HTTP gateway root span
            } else {
                1 // INTERNAL
            }
        }
    }
}

pub(crate) fn wrapped_function_type_name(t: &WrappedFunctionType) -> &'static str {
    match t {
        WrappedFunctionType::ReadLocal => "read-local",
        WrappedFunctionType::WriteLocal => "write-local",
        WrappedFunctionType::ReadRemote => "read-remote",
        WrappedFunctionType::WriteRemote => "write-remote",
        WrappedFunctionType::WriteRemoteBatched(_) => "write-remote-batched",
        WrappedFunctionType::WriteRemoteTransaction(_) => "write-remote-transaction",
    }
}

pub(crate) fn oplog_payload_size(
    payload: &golem_rust::bindings::golem::api::oplog::OplogPayload,
) -> Option<u64> {
    match payload {
        golem_rust::bindings::golem::api::oplog::OplogPayload::Inline(data) => {
            Some(data.len() as u64)
        }
        golem_rust::bindings::golem::api::oplog::OplogPayload::External(_) => None,
    }
}

pub(crate) fn worker_error_to_string(e: &WorkerError) -> String {
    match e {
        WorkerError::Unknown(s) => format!("unknown: {s}"),
        WorkerError::InvalidRequest(s) => format!("invalid request: {s}"),
        WorkerError::StackOverflow => "stack overflow".to_string(),
        WorkerError::OutOfMemory => "out of memory".to_string(),
        WorkerError::ExceededMemoryLimit => "exceeded memory limit".to_string(),
        WorkerError::InternalError(s) => format!("internal error: {s}"),
        WorkerError::ExceededTableLimit => todo!(),
        WorkerError::NodeOutOfFilesystemStorage => todo!(),
        WorkerError::AgentExceededFilesystemStorageLimit => todo!(),
    }
}
