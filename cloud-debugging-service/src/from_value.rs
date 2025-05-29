use golem_api_grpc::proto::golem::worker::UpdateMode;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::{ComponentId, ComponentVersion, PromiseId, WorkerId};
use golem_wasm_rpc::Value;
use golem_worker_executor::durable_host::http::serialized::{
    SerializableDnsErrorPayload, SerializableErrorCode, SerializableFieldSizePayload,
    SerializableHttpMethod, SerializableHttpRequest, SerializableResponse,
    SerializableResponseHeaders, SerializableTlsAlertReceivedPayload,
};
use golem_worker_executor::durable_host::serialized::{
    SerializableDateTime, SerializableError, SerializableFileTimes, SerializableIpAddress,
    SerializableIpAddresses, SerializableStreamError,
};
use golem_worker_executor::error::GolemError;
use golem_worker_executor::model::InterruptKind;
use golem_worker_executor::services::blob_store::ObjectMetadata;
use golem_worker_executor::services::rpc::RpcError;
use golem_worker_executor::services::worker_proxy::WorkerProxyError;
use std::collections::HashMap;
use std::net::IpAddr;
use std::ops::Deref;
use std::str::FromStr;
use uuid::Uuid;

// A reverse of IntoValue, but not being aware of Type
pub trait FromValue {
    fn from_value(value: &Value) -> Result<Self, String>
    where
        Self: Sized;
}

impl FromValue for Value {
    fn from_value(value: &Value) -> Result<Self, String>
    where
        Self: Sized,
    {
        Ok(value.clone())
    }
}

impl FromValue for SerializableHttpRequest {
    fn from_value(value: &Value) -> Result<Self, String>
    where
        Self: Sized,
    {
        match value {
            Value::Record(values) => {
                let uri = &values[0];
                let method = &values[1];
                let headers = &values[2];

                let uri = String::from_value(uri)?;
                let method = SerializableHttpMethod::from_value(method)?;
                let headers: HashMap<String, String> = HashMap::from_value(headers)?;

                Ok(SerializableHttpRequest {
                    uri,
                    method,
                    headers,
                })
            }
            _ => Err("Failed to get SerializableHttpRequest from Value".to_string()),
        }
    }
}

impl FromValue for SerializableHttpMethod {
    fn from_value(value: &Value) -> Result<Self, String>
    where
        Self: Sized,
    {
        match value {
            Value::Variant {
                case_idx,
                case_value,
            } => match (case_idx, case_value) {
                (0, None) => Ok(SerializableHttpMethod::Get),
                (1, None) => Ok(SerializableHttpMethod::Post),
                (2, None) => Ok(SerializableHttpMethod::Put),
                (3, None) => Ok(SerializableHttpMethod::Delete),
                (4, None) => Ok(SerializableHttpMethod::Head),
                (5, None) => Ok(SerializableHttpMethod::Connect),
                (6, None) => Ok(SerializableHttpMethod::Options),
                (7, None) => Ok(SerializableHttpMethod::Trace),
                (8, None) => Ok(SerializableHttpMethod::Patch),
                (9, Some(value)) => {
                    let string = String::from_value(value)?;
                    Ok(SerializableHttpMethod::Other(string))
                }
                _ => Err("Failed to get SerializableHttpMethod from Value".to_string()),
            },

            _ => Err(
                "Failed to get SerializableHttpMethod from Value. Value is not a Variant"
                    .to_string(),
            ),
        }
    }
}

impl FromValue for SerializableDateTime {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::Record(values) => {
                if values.len() != 2 {
                    Err("Failed to get component id from Value".to_string())
                } else {
                    let seconds = &values[0];
                    let nano_seconds = &values[1];
                    let seconds = u64::from_value(seconds)?;
                    let nanoseconds = u32::from_value(nano_seconds)?;
                    Ok(SerializableDateTime {
                        seconds,
                        nanoseconds,
                    })
                }
            }

            _ => Err("Failed to get SerializableDateTime from Value".to_string()),
        }
    }
}

impl FromValue for SerializableFileTimes {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::Record(values) => {
                if values.len() != 2 {
                    Err("Failed to get component id from Value".to_string())
                } else {
                    let data_access_timestamp = &values[0];
                    let data_modification_timestamp = &values[1];
                    let data_access_timestamp: Option<SerializableDateTime> =
                        Option::from_value(data_access_timestamp)?;
                    let data_modification_timestamp: Option<SerializableDateTime> =
                        Option::from_value(data_modification_timestamp)?;
                    Ok(SerializableFileTimes {
                        data_access_timestamp,
                        data_modification_timestamp,
                    })
                }
            }

            _ => Err("Failed to get SerializableFileTimes from Value".to_string()),
        }
    }
}

impl FromValue for ComponentId {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::Record(inner_records) => {
                if inner_records.len() != 1 {
                    return Err("Failed to get component id from Value".to_string());
                };

                let value = &inner_records[0];

                match value {
                    Value::Record(values) => {
                        if values.len() != 2 {
                            Err("Failed to get component id from Value".to_string())
                        } else {
                            let hi = &values[0];
                            let low = &values[1];
                            let hi = u64::from_value(hi)?;
                            let low = u64::from_value(low)?;
                            let uuid = Uuid::from_u64_pair(hi, low);
                            Ok(ComponentId(uuid))
                        }
                    }

                    _ => Err("Failed to get component id from Value".to_string()),
                }
            }

            _ => Err("Failed to get component id".to_string()),
        }
    }
}

// Cannot reuse for structures like component-id, which uses Value::Record internally
impl FromValue for Uuid {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::String(uuid) => Uuid::from_str(uuid).map_err(|err| err.to_string()),
            _ => Err("Failed to obtain UUID from Value".to_string()),
        }
    }
}

impl FromValue for WorkerId {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::Record(values) => {
                if values.len() != 2 {
                    Err("Failed to get worker-id from value".to_string())
                } else {
                    let component_id = &values[0];
                    let worker_name = &values[1];

                    let component_id = ComponentId::from_value(component_id)?;
                    let worker_name = String::from_value(worker_name)?;
                    Ok(WorkerId {
                        component_id,
                        worker_name,
                    })
                }
            }

            _ => Err("Failed to get worker id from value".to_string()),
        }
    }
}

impl FromValue for ObjectMetadata {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::Record(values) => {
                if values.len() < 4 {
                    return Err("Failed to get object metadata".to_string());
                }
                let name = &values[0];
                let container = &values[1];
                let created_at = &values[2];
                let size = &values[3];

                let name = String::from_value(name)?;
                let container = String::from_value(container)?;
                let created_at = u64::from_value(created_at)?;
                let size = u64::from_value(size)?;
                Ok(ObjectMetadata {
                    name,
                    container,
                    created_at,
                    size,
                })
            }

            _ => Err("Failed to get object-metadata from value".to_string()),
        }
    }
}

impl FromValue for SerializableIpAddress {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::String(ip_address_text) => {
                let ip_address = ip_address_text
                    .parse::<IpAddr>()
                    .map_err(|err| err.to_string())?;
                match ip_address {
                    IpAddr::V4(ip_v4) => Ok(SerializableIpAddress::IPv4 {
                        address: ip_v4.octets(),
                    }),
                    IpAddr::V6(ip_v6) => Ok(SerializableIpAddress::IPv6 {
                        address: ip_v6.segments(),
                    }),
                }
            }

            _ => Err("Failed to get serializable ip address from value".to_string()),
        }
    }
}

impl FromValue for PromiseId {
    fn from_value(value: &Value) -> Result<Self, String>
    where
        Self: Sized,
    {
        match value {
            Value::Record(rec) => {
                if rec.len() != 2 {
                    Err("Failed to get PromiseId from Value".to_string())
                } else {
                    let worker_id: WorkerId = WorkerId::from_value(&rec[0])?;
                    let oplog_idx: OplogIndex = OplogIndex::from_u64(u64::from_value(&rec[1])?);
                    Ok(PromiseId {
                        worker_id,
                        oplog_idx,
                    })
                }
            }
            _ => Err("Failed to get PromiseId from Value".to_string()),
        }
    }
}

impl FromValue for InterruptKind {
    fn from_value(value: &Value) -> Result<Self, String>
    where
        Self: Sized,
    {
        match value {
            Value::Enum(num) => match num {
                0 => Ok(InterruptKind::Interrupt),
                1 => Ok(InterruptKind::Restart),
                2 => Ok(InterruptKind::Suspend),
                3 => Ok(InterruptKind::Jump),
                _ => Err("Failed to get interrupt-kind from value".to_string()),
            },
            _ => Err("Failed to get interrupt-kind from value".to_string()),
        }
    }
}

impl FromValue for SerializableIpAddresses {
    fn from_value(value: &Value) -> Result<Self, String> {
        let value: Vec<SerializableIpAddress> = Vec::from_value(value)?;
        Ok(SerializableIpAddresses(value))
    }
}

impl FromValue for SerializableStreamError {
    fn from_value(value: &Value) -> Result<Self, String> {
        if let Value::Variant {
            case_idx,
            case_value,
        } = value
        {
            match (case_idx, case_value) {
                (0, None) => Ok(SerializableStreamError::Closed),
                (1, Some(payload_value)) => {
                    let error = SerializableError::from_value(payload_value)?;
                    Ok(SerializableStreamError::LastOperationFailed(error))
                }
                (2, Some(payload_value)) => {
                    let error = SerializableError::from_value(payload_value)?;
                    Ok(SerializableStreamError::Trap(error))
                }

                _ => Err("Failed to get SerializableStreamError from variant".to_string()),
            }
        } else {
            Err("Failed to get SerializableStreamError from value".to_string())
        }
    }
}

impl FromValue for GolemError {
    fn from_value(value: &Value) -> Result<Self, String>
    where
        Self: Sized,
    {
        match value {
            Value::Variant {
                case_idx,
                case_value,
            } => match (case_idx, case_value) {
                (0, Some(error)) => match error.deref() {
                    Value::Record(errors) => {
                        if errors.len() != 1 {
                            return Err("Failed to get GolemError".to_string());
                        }

                        let error = &errors[0];
                        let error_str = String::from_value(error)?;
                        Ok(GolemError::invalid_request(error_str))
                    }

                    _ => Err("Failed to get GolemError. Not a Record".to_string()),
                },

                (1, Some(error)) => match error.deref() {
                    Value::Record(values) => {
                        if values.len() != 1 {
                            return Err("Failed to get GolemError".to_string());
                        }

                        let worker_id_val = &values[0];
                        let worker_id = WorkerId::from_value(worker_id_val)?;
                        Ok(GolemError::worker_already_exists(worker_id))
                    }

                    _ => Err("Failed to get GolemError. Not a Record".to_string()),
                },

                (2, Some(error)) => match error.deref() {
                    Value::Record(values) => {
                        if values.len() != 1 {
                            return Err("Failed to get GolemError".to_string());
                        }

                        let worker_id_val = &values[0];
                        let worker_id = WorkerId::from_value(worker_id_val)?;
                        Ok(GolemError::worker_not_found(worker_id))
                    }

                    _ => Err("Failed to get GolemError. Not a Record".to_string()),
                },
                (3, Some(error)) => match error.deref() {
                    Value::Record(values) => {
                        if values.len() != 2 {
                            return Err("Failed to get GolemError".to_string());
                        }

                        let worker_id_val = &values[0];
                        let details_val = &values[1];
                        let worker_id = WorkerId::from_value(worker_id_val)?;
                        let details = String::from_value(details_val)?;
                        Ok(GolemError::worker_creation_failed(worker_id, details))
                    }

                    _ => Err("Failed to get GolemError. Not a Record".to_string()),
                },

                (4, Some(error)) => match error.deref() {
                    Value::Record(values) => {
                        if values.len() != 2 {
                            return Err("Failed to get GolemError".to_string());
                        }

                        let worker_id_val = &values[0];
                        let details_val = &values[1];
                        let worker_id = WorkerId::from_value(worker_id_val)?;
                        let golem_error = GolemError::from_value(details_val).or_else(|_| {
                            String::from_value(details_val).map(GolemError::unknown)
                        })?;

                        Ok(GolemError::failed_to_resume_worker(worker_id, golem_error))
                    }

                    _ => Err("Failed to get GolemError. Not a Record".to_string()),
                },

                (5, Some(error)) => match error.deref() {
                    Value::Record(values) => {
                        if values.len() != 3 {
                            return Err("Failed to get GolemError".to_string());
                        }

                        let component_id = &values[0];
                        let component_version = &values[1];
                        let reason = &values[2];
                        let component_id = ComponentId::from_value(component_id)?;
                        let component_version = u64::from_value(component_version)?;
                        let reason = String::from_value(reason)?;

                        Ok(GolemError::component_download_failed(
                            component_id,
                            component_version,
                            reason,
                        ))
                    }

                    _ => Err("Failed to get GolemError. Not a Record".to_string()),
                },

                (6, Some(error)) => match error.deref() {
                    Value::Record(values) => {
                        if values.len() != 3 {
                            return Err("Failed to get GolemError".to_string());
                        }

                        let component_id = &values[0];
                        let component_version = &values[1];
                        let reason = &values[2];
                        let component_id = ComponentId::from_value(component_id)?;
                        let component_version = u64::from_value(component_version)?;
                        let reason = String::from_value(reason)?;

                        Ok(GolemError::ComponentParseFailed {
                            component_id,
                            component_version,
                            reason,
                        })
                    }

                    _ => Err("Failed to get GolemError. Not a Record".to_string()),
                },

                (7, Some(error)) => match error.deref() {
                    Value::Record(values) => {
                        if values.len() != 2 {
                            return Err("Failed to get GolemError".to_string());
                        }

                        let component_id = &values[0];
                        let reason = &values[1];
                        let component_id = ComponentId::from_value(component_id)?;
                        let reason = String::from_value(reason)?;

                        Ok(GolemError::GetLatestVersionOfComponentFailed {
                            component_id,
                            reason,
                        })
                    }

                    _ => Err("Failed to get GolemError. Not a Record".to_string()),
                },

                (8, Some(error)) => match error.deref() {
                    Value::Record(values) => {
                        if values.len() != 1 {
                            return Err("Failed to get GolemError".to_string());
                        }

                        let promise_id_val = &values[0];
                        let promise_id = PromiseId::from_value(promise_id_val)?;

                        Ok(GolemError::PromiseNotFound { promise_id })
                    }

                    _ => Err("Failed to get GolemError. Not a Record".to_string()),
                },
                (9, Some(error)) => match error.deref() {
                    Value::Record(values) => {
                        if values.len() != 1 {
                            return Err("Failed to get GolemError".to_string());
                        }

                        let promise_id_val = &values[0];
                        let promise_id = PromiseId::from_value(promise_id_val)?;

                        Ok(GolemError::PromiseDropped { promise_id })
                    }

                    _ => Err("Failed to get GolemError. Not a Record".to_string()),
                },
                (10, Some(error)) => match error.deref() {
                    Value::Record(values) => {
                        if values.len() != 1 {
                            return Err("Failed to get GolemError".to_string());
                        }

                        let promise_id_val = &values[0];
                        let promise_id = PromiseId::from_value(promise_id_val)?;

                        Ok(GolemError::PromiseAlreadyCompleted { promise_id })
                    }

                    _ => Err("Failed to get GolemError. Not a Record".to_string()),
                },

                (11, Some(error)) => match error.deref() {
                    Value::Record(values) => {
                        if values.len() != 1 {
                            return Err("Failed to get GolemError".to_string());
                        }

                        let interrupt_kind_val = &values[0];
                        let kind = InterruptKind::from_value(interrupt_kind_val)?;

                        Ok(GolemError::Interrupted { kind })
                    }

                    _ => Err("Failed to get GolemError. Not a Record".to_string()),
                },
                (12, Some(error)) => match error.deref() {
                    Value::Record(values) => {
                        if values.len() != 1 {
                            return Err("Failed to get GolemError".to_string());
                        }

                        let details = String::from_value(&values[0])?;

                        Ok(GolemError::ParamTypeMismatch { details })
                    }

                    _ => Err("Failed to get GolemError. Not a Record".to_string()),
                },
                (13, None) => Ok(GolemError::NoValueInMessage),
                (14, Some(error)) => match error.deref() {
                    Value::Record(values) => {
                        if values.len() != 1 {
                            return Err("Failed to get GolemError".to_string());
                        }

                        let details = String::from_value(&values[0])?;

                        Ok(GolemError::ValueMismatch { details })
                    }

                    _ => Err("Failed to get GolemError. Not a Record".to_string()),
                },

                (15, Some(error)) => match error.deref() {
                    Value::Record(values) => {
                        if values.len() != 2 {
                            return Err("Failed to get GolemError".to_string());
                        }

                        let expected = &values[0];
                        let got = &values[1];
                        let expected = String::from_value(expected)?;
                        let got = String::from_value(got)?;

                        Ok(GolemError::UnexpectedOplogEntry { expected, got })
                    }

                    _ => Err("Failed to get GolemError. Not a Record".to_string()),
                },
                (16, Some(error)) => match error.deref() {
                    Value::Record(values) => {
                        if values.len() != 1 {
                            return Err("Failed to get GolemError".to_string());
                        }

                        let details = String::from_value(&values[0])?;

                        Ok(GolemError::Runtime { details })
                    }

                    _ => Err("Failed to get GolemError. Not a Record".to_string()),
                },
                (17, Some(error)) => match error.deref() {
                    Value::Record(values) => {
                        if values.len() != 2 {
                            return Err("Failed to get GolemError".to_string());
                        }

                        let expected = &values[0];
                        let got = &values[1];
                        let expected = String::from_value(expected)?;
                        let got = String::from_value(got)?;

                        Ok(GolemError::UnexpectedOplogEntry { expected, got })
                    }

                    _ => Err("Failed to get GolemError. Not a Record".to_string()),
                },
                (18, None) => Ok(GolemError::InvalidAccount),
                (19, Some(error)) => match error.deref() {
                    Value::Record(values) => {
                        if values.len() != 1 {
                            return Err("Failed to get GolemError".to_string());
                        }

                        let details = String::from_value(&values[0])?;

                        Ok(GolemError::PreviousInvocationFailed { details })
                    }

                    _ => Err("Failed to get GolemError. Not a Record".to_string()),
                },
                (20, None) => Ok(GolemError::PreviousInvocationExited),
                (21, Some(error)) => match error.deref() {
                    Value::Record(values) => {
                        if values.len() != 1 {
                            return Err("Failed to get GolemError".to_string());
                        }

                        let details = String::from_value(&values[0])?;

                        Ok(GolemError::Unknown { details })
                    }

                    _ => Err("Failed to get GolemError. Not a Record".to_string()),
                },
                (22, None) => Ok(GolemError::ShardingNotReady),
                (23, Some(error)) => match error.deref() {
                    Value::Record(values) => {
                        if values.len() != 2 {
                            return Err("Failed to get GolemError".to_string());
                        }

                        let path = &values[0];
                        let reason = &values[1];
                        let path = String::from_value(path)?;
                        let reason = String::from_value(reason)?;

                        Ok(GolemError::InitialComponentFileDownloadFailed { path, reason })
                    }

                    _ => Err("Failed to get GolemError. Not a Record".to_string()),
                },
                (24, Some(error)) => match error.deref() {
                    Value::Record(values) => {
                        if values.len() != 2 {
                            return Err("Failed to get GolemError".to_string());
                        }

                        let path = &values[0];
                        let reason = &values[1];
                        let path = String::from_value(path)?;
                        let reason = String::from_value(reason)?;

                        Ok(GolemError::FileSystemError { path, reason })
                    }

                    _ => Err("Failed to get GolemError. Not a Record".to_string()),
                },

                _ => Err("Failed to get GolemError. Not a Record".to_string()),
            },

            _ => Err("failed to get golem error".to_string()),
        }
    }
}

impl FromValue for WorkerProxyError {
    fn from_value(value: &Value) -> Result<Self, String>
    where
        Self: Sized,
    {
        match value {
            Value::Variant {
                case_idx,
                case_value,
            } => match (case_idx, case_value) {
                (0, Some(errors)) => {
                    let errors: Vec<String> = Vec::from_value(errors)?;
                    Ok(WorkerProxyError::BadRequest(errors))
                }
                (1, Some(errors)) => {
                    let errors: String = String::from_value(errors)?;
                    Ok(WorkerProxyError::Unauthorized(errors))
                }
                (2, Some(errors)) => {
                    let errors: String = String::from_value(errors)?;
                    Ok(WorkerProxyError::LimitExceeded(errors))
                }
                (3, Some(errors)) => {
                    let errors: String = String::from_value(errors)?;
                    Ok(WorkerProxyError::NotFound(errors))
                }
                (4, Some(errors)) => {
                    let errors: String = String::from_value(errors)?;
                    Ok(WorkerProxyError::AlreadyExists(errors))
                }
                (5, Some(errors)) => {
                    let errors: GolemError = GolemError::from_value(errors)?;
                    Ok(WorkerProxyError::InternalError(errors))
                }

                _ => Err("Failed to get worker proxy error from Value".to_string()),
            },
            _ => Err("Failed to get worker proxy error from Value".to_string()),
        }
    }
}

impl FromValue for SerializableError {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::Variant {
                case_idx,
                case_value,
            } => match (case_idx, case_value) {
                (0, Some(payload)) => {
                    let error = String::from_value(payload)?;
                    Ok(SerializableError::Generic { message: error })
                }
                (1, Some(payload)) => {
                    let error = u8::from_value(payload)?;
                    Ok(SerializableError::FsError { code: error })
                }
                (2, Some(golem_error_value)) => {
                    let error = GolemError::from_value(golem_error_value)?;
                    Ok(SerializableError::Golem { error })
                }
                (3, Some(payload)) => {
                    let error = u8::from_value(payload)?;
                    Ok(SerializableError::SocketError { code: error })
                }
                (4, Some(payload)) => {
                    let error = RpcError::from_value(payload)?;
                    Ok(SerializableError::Rpc { error })
                }
                (5, Some(payload)) => {
                    let error = WorkerProxyError::from_value(payload)?;
                    Ok(SerializableError::WorkerProxy { error })
                }
                _ => Err("Failed to get SerializableError from Value".to_string()),
            },

            _ => Err("Failed to get SerializableError from Value".to_string()),
        }
    }
}

impl FromValue for RpcError {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::Variant {
                case_idx,
                case_value,
            } => match (case_idx, case_value) {
                (0, Some(details)) => {
                    let details = String::from_value(details).unwrap();
                    Ok(RpcError::ProtocolError { details })
                }
                (1, Some(details)) => {
                    let details = String::from_value(details).unwrap();
                    Ok(RpcError::Denied { details })
                }
                (2, Some(details)) => {
                    let details = String::from_value(details).unwrap();
                    Ok(RpcError::NotFound { details })
                }
                (3, Some(details)) => {
                    let details = String::from_value(details).unwrap();
                    Ok(RpcError::RemoteInternalError { details })
                }
                _ => Err("Expected Value to be Variant to obtain RpcError".to_string()),
            },
            _ => Err("Expected Value to be Variant to obtain RpcError".to_string()),
        }
    }
}

impl FromValue for SerializableResponse {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::Variant {
                case_idx,
                case_value,
            } => match (case_idx, case_value) {
                (0, None) => Ok(SerializableResponse::Pending),
                (1, Some(headers)) => {
                    let headers = SerializableResponseHeaders::from_value(headers)?;
                    Ok(SerializableResponse::HeadersReceived(headers))
                }
                (2, Some(body)) => {
                    let error_code = SerializableErrorCode::from_value(body)?;
                    Ok(SerializableResponse::HttpError(error_code))
                }
                (3, Some(body)) => {
                    let error_code = Option::from_value(body)?;
                    Ok(SerializableResponse::InternalError(error_code))
                }
                _ => Err("Invalid case_idx for SerializableResponse".to_string()),
            },

            _ => Err("Expected Value to be Variant to obtain SerializableResponse".to_string()),
        }
    }
}

impl FromValue for SerializableResponseHeaders {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::Record(record) => {
                if record.len() != 2 {
                    Err("Expected record with 2 fields".to_string())
                } else {
                    let status = &record[0];
                    let headers = &record[1];

                    let status_value = u16::from_value(status)?;
                    let headers_value: HashMap<String, String> = HashMap::from_value(headers)?;
                    let headers_value_in_bytes: HashMap<String, Vec<u8>> = headers_value
                        .into_iter()
                        .map(|(k, v)| (k, v.into_bytes()))
                        .collect();

                    Ok(SerializableResponseHeaders {
                        status: status_value,
                        headers: headers_value_in_bytes,
                    })
                }
            }
            _ => {
                Err("Expected Value to be Record to obtain SerializableResponseHeaders".to_string())
            }
        }
    }
}

impl FromValue for String {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::String(string) => Ok(string.clone()),
            _ => Err("Expected String".to_string()),
        }
    }
}

impl FromValue for i64 {
    fn from_value(value: &Value) -> Result<Self, String>
    where
        Self: Sized,
    {
        match value {
            Value::S64(i64) => Ok(*i64),
            _ => Err("Cannot get i64 from Value".to_string()),
        }
    }
}

impl FromValue for u64 {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::U64(u64) => Ok(*u64),
            _ => Err("Expected u64".to_string()),
        }
    }
}

impl FromValue for u32 {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::U32(u32) => Ok(*u32),
            _ => Err("Expected U32".to_string()),
        }
    }
}

impl FromValue for u16 {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::U16(u16) => Ok(*u16),
            _ => Err("Expected U16".to_string()),
        }
    }
}

impl FromValue for u8 {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::U8(u8) => Ok(*u8),
            _ => Err("Expected U8".to_string()),
        }
    }
}

impl FromValue for bool {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::Bool(bool) => Ok(*bool),
            _ => Err("Expected bool".to_string()),
        }
    }
}

impl<T: FromValue> FromValue for Vec<T> {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::List(vec) => vec.iter().map(|x| T::from_value(x)).collect(),
            _ => Err("Failed to get vec<T> from value".to_string()),
        }
    }
}

pub struct Tuple<K, V>(K, V);

impl<K: FromValue, V: FromValue> FromValue for (K, V) {
    fn from_value(value: &Value) -> Result<Self, String> {
        let result: Tuple<K, V> = Tuple::from_value(value)?;
        Ok((result.0, result.1))
    }
}

impl<K: FromValue, V: FromValue> FromValue for Tuple<K, V> {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::Tuple(vec) => {
                let k: K = K::from_value(&vec[0])?;
                let v: V = V::from_value(&vec[1])?;
                Ok(Tuple(k, v))
            }
            _ => Err("Failed to get tuple from Value".to_string()),
        }
    }
}

impl<K: FromValue + std::hash::Hash + Eq, V: FromValue> FromValue for HashMap<K, V> {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::List(collection) => collection
                .iter()
                .map(|x| {
                    let tuple: Tuple<K, V> = Tuple::from_value(x)?;
                    Ok((tuple.0, tuple.1))
                })
                .collect::<Result<HashMap<K, V>, String>>(),

            _ => Err("Failed to get hashmap from Value".to_string()),
        }
    }
}

impl<T: FromValue> FromValue for Option<T> {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::Option(optional) => match optional {
                None => Ok(None),
                Some(value) => {
                    let t = T::from_value(value)?;
                    Ok(Some(t))
                }
            },
            _ => Err("Expected Value to be Option to obtain Option<T>".to_string()),
        }
    }
}

impl<E: FromValue> FromValue for Result<(), E> {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::Result(Ok(None)) => Ok(Ok(())),
            Value::Result(Err(Some(value))) => {
                let e = E::from_value(value)?;
                Ok(Err(e))
            }
            _ => Err("Expected Value to be Result to obtain Result<(), E>".to_string()),
        }
    }
}

impl<S: FromValue> FromValue for Result<S, ()> {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::Result(Ok(Some(value))) => {
                let s = S::from_value(value)?;
                Ok(Ok(s))
            }
            Value::Result(Err(None)) => Ok(Err(())),
            _ => Err("Expected Value to be Result to obtain Result<S, ()>".to_string()),
        }
    }
}

impl<S: FromValue, E: FromValue> FromValue for Result<S, E> {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::Result(Ok(Some(value))) => {
                let s = S::from_value(value)?;
                Ok(Ok(s))
            }
            Value::Result(Err(Some(value))) => {
                let e = E::from_value(value)?;
                Ok(Err(e))
            }
            _ => Err("Expected Value to be Result to obtain Result<S, E>".to_string()),
        }
    }
}

impl FromValue for SerializableDnsErrorPayload {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::Record(record) => {
                if record.len() != 2 {
                    Err("Expected record with 2 fields".to_string())
                } else {
                    let rcode_value = &record[0];
                    let info_code_value = &record[0];
                    let rcode: Option<String> = Option::from_value(rcode_value)?;
                    let info_code: Option<u16> = Option::from_value(info_code_value)?;

                    Ok(SerializableDnsErrorPayload { rcode, info_code })
                }
            }
            _ => {
                Err("Expected Value to be Record to obtain SerializableDnsErrorPayload".to_string())
            }
        }
    }
}

impl FromValue for SerializableTlsAlertReceivedPayload {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::Record(record) => {
                if record.len() != 2 {
                    Err("Expected record with 2 fields".to_string())
                } else {
                    let alert_id_value = &record[0];
                    let alert_message_value = &record[0];
                    let alert_id: Option<u8> = Option::from_value(alert_id_value)?;
                    let alert_message: Option<String> = Option::from_value(alert_message_value)?;

                    Ok(SerializableTlsAlertReceivedPayload {
                        alert_id,
                        alert_message,
                    })
                }
            }
            _ => Err(
                "Expected Value to be Record to obtain SerializableTlsAlertReceivedPayload"
                    .to_string(),
            ),
        }
    }
}

impl FromValue for SerializableFieldSizePayload {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::Record(record) => {
                if record.len() != 2 {
                    Err("Expected record with 2 fields".to_string())
                } else {
                    let field_name_value = &record[0];
                    let field_size_value = &record[0];
                    let field_name: Option<String> = Option::from_value(field_name_value)?;
                    let field_size: Option<u32> = Option::from_value(field_size_value)?;

                    Ok(SerializableFieldSizePayload {
                        field_name,
                        field_size,
                    })
                }
            }
            _ => Err(
                "Expected Value to be Record to obtain SerializableFieldSizePayload".to_string(),
            ),
        }
    }
}

impl FromValue for SerializableErrorCode {
    fn from_value(value: &Value) -> Result<Self, String> {
        match value {
            Value::Variant {
                case_idx,
                case_value,
            } => match (case_idx, case_value) {
                (0, None) => Ok(SerializableErrorCode::DnsTimeout),
                (1, Some(payload)) => {
                    let dns_payload = SerializableDnsErrorPayload::from_value(payload)?;
                    Ok(SerializableErrorCode::DnsError(dns_payload))
                }
                (2, None) => Ok(SerializableErrorCode::DestinationNotFound),
                (3, None) => Ok(SerializableErrorCode::DestinationUnavailable),
                (4, None) => Ok(SerializableErrorCode::DestinationIpProhibited),
                (5, None) => Ok(SerializableErrorCode::DestinationIpUnroutable),
                (6, None) => Ok(SerializableErrorCode::ConnectionRefused),
                (7, None) => Ok(SerializableErrorCode::ConnectionTerminated),
                (8, None) => Ok(SerializableErrorCode::ConnectionTimeout),
                (9, None) => Ok(SerializableErrorCode::ConnectionReadTimeout),
                (10, None) => Ok(SerializableErrorCode::ConnectionWriteTimeout),
                (11, None) => Ok(SerializableErrorCode::ConnectionLimitReached),
                (12, None) => Ok(SerializableErrorCode::TlsProtocolError),
                (13, None) => Ok(SerializableErrorCode::TlsCertificateError),
                (14, Some(payload)) => {
                    let tls_alert_received =
                        SerializableTlsAlertReceivedPayload::from_value(payload)?;
                    Ok(SerializableErrorCode::TlsAlertReceived(tls_alert_received))
                }
                (15, None) => Ok(SerializableErrorCode::HttpRequestDenied),

                (16, None) => Ok(SerializableErrorCode::HttpRequestLengthRequired),

                (17, Some(payload)) => {
                    let size: Option<u64> = Option::from_value(payload)?;
                    Ok(SerializableErrorCode::HttpRequestBodySize(size))
                }

                (18, None) => Ok(SerializableErrorCode::HttpRequestMethodInvalid),
                (19, None) => Ok(SerializableErrorCode::HttpRequestUriInvalid),
                (20, None) => Ok(SerializableErrorCode::HttpRequestUriTooLong),
                (21, Some(payload)) => {
                    let size: Option<u32> = Option::from_value(payload)?;
                    Ok(SerializableErrorCode::HttpRequestHeaderSectionSize(size))
                }
                (22, Some(payload)) => {
                    let size: Option<SerializableFieldSizePayload> = Option::from_value(payload)?;
                    Ok(SerializableErrorCode::HttpRequestHeaderSize(size))
                }

                (23, Some(payload)) => {
                    let size: Option<u32> = Option::from_value(payload)?;
                    Ok(SerializableErrorCode::HttpRequestTrailerSectionSize(size))
                }
                (24, Some(payload)) => {
                    let size = SerializableFieldSizePayload::from_value(payload)?;
                    Ok(SerializableErrorCode::HttpRequestTrailerSize(size))
                }

                (25, None) => Ok(SerializableErrorCode::HttpResponseIncomplete),

                (26, Some(payload)) => {
                    let size: Option<u32> = Option::from_value(payload)?;

                    Ok(SerializableErrorCode::HttpResponseHeaderSectionSize(size))
                }

                (27, Some(payload)) => {
                    let size = SerializableFieldSizePayload::from_value(payload)?;

                    Ok(SerializableErrorCode::HttpResponseHeaderSize(size))
                }

                (28, Some(payload)) => {
                    let size: Option<u64> = Option::from_value(payload)?;
                    Ok(SerializableErrorCode::HttpResponseBodySize(size))
                }

                (29, Some(payload)) => {
                    let size: Option<u32> = Option::from_value(payload)?;
                    Ok(SerializableErrorCode::HttpResponseTrailerSectionSize(size))
                }

                (30, Some(payload)) => {
                    let size = SerializableFieldSizePayload::from_value(payload)?;

                    Ok(SerializableErrorCode::HttpResponseTrailerSize(size))
                }

                (31, Some(payload)) => {
                    let size: Option<String> = Option::from_value(payload)?;
                    Ok(SerializableErrorCode::HttpResponseTransferCoding(size))
                }

                (32, Some(payload)) => {
                    let size: Option<String> = Option::from_value(payload)?;
                    Ok(SerializableErrorCode::HttpResponseContentCoding(size))
                }

                (33, None) => Ok(SerializableErrorCode::HttpResponseTimeout),

                (34, None) => Ok(SerializableErrorCode::HttpUpgradeFailed),

                (35, None) => Ok(SerializableErrorCode::HttpProtocolError),

                (36, None) => Ok(SerializableErrorCode::LoopDetected),

                (37, None) => Ok(SerializableErrorCode::ConfigurationError),

                (38, Some(payload)) => {
                    let size: Option<String> = Option::from_value(payload)?;
                    Ok(SerializableErrorCode::InternalError(size))
                }

                _ => Err("Invalid case_idx for SerializableErrorCode".to_string()),
            },
            _ => Err("Expected Value to be Variant to obtain SerializableErrorCode".to_string()),
        }
    }
}

pub struct Container(pub String);

impl FromValue for Container {
    fn from_value(value: &Value) -> Result<Self, String>
    where
        Self: Sized,
    {
        match value {
            Value::Record(values) => {
                if values.len() != 1 {
                    Err("Failed to get Container from Value".to_string())
                } else {
                    let container_str = String::from_value(&values[0])?;

                    Ok(Container(container_str))
                }
            }

            _ => Err("Failed to get Container from Value".to_string()),
        }
    }
}

pub struct ContainerAndObject {
    pub container: String,
    pub object: String,
}

impl FromValue for ContainerAndObject {
    fn from_value(value: &Value) -> Result<Self, String>
    where
        Self: Sized,
    {
        match value {
            Value::Record(values) => {
                if values.len() != 2 {
                    Err("Failed to get Container from Value".to_string())
                } else {
                    let container = String::from_value(&values[0])?;
                    let object = String::from_value(&values[1])?;

                    Ok(ContainerAndObject { container, object })
                }
            }

            _ => Err("Failed to get ContainerAndObject from Value".to_string()),
        }
    }
}

pub struct ContainerObjectBeginEnd {
    pub container: String,
    pub object: String,
    pub begin: u64,
    pub end: u64,
}

impl FromValue for ContainerObjectBeginEnd {
    fn from_value(value: &Value) -> Result<Self, String>
    where
        Self: Sized,
    {
        match value {
            Value::Record(values) => {
                if values.len() != 4 {
                    Err("Failed to get ContainerObjectBeginEnd from Value".to_string())
                } else {
                    let container = String::from_value(&values[0])?;
                    let object = String::from_value(&values[1])?;
                    let begin = u64::from_value(&values[2])?;
                    let end = u64::from_value(&values[3])?;

                    Ok(ContainerObjectBeginEnd {
                        container,
                        object,
                        begin,
                        end,
                    })
                }
            }

            _ => Err("Failed to get ContainerObjectBeginEnd from Value".to_string()),
        }
    }
}

pub struct ContainerCopyObjectInfo {
    pub src_container: String,
    pub src_object: String,
    pub dest_container: String,
    pub dest_object: String,
}

impl FromValue for ContainerCopyObjectInfo {
    fn from_value(value: &Value) -> Result<Self, String>
    where
        Self: Sized,
    {
        match value {
            Value::Record(values) => {
                if values.len() != 4 {
                    Err("Failed to get ContainerCopyObjectInfo from Value".to_string())
                } else {
                    let src_container = String::from_value(&values[0])?;
                    let src_object = String::from_value(&values[1])?;
                    let dest_container = String::from_value(&values[2])?;
                    let dest_object = String::from_value(&values[3])?;

                    Ok(ContainerCopyObjectInfo {
                        src_container,
                        src_object,
                        dest_container,
                        dest_object,
                    })
                }
            }

            _ => Err("Failed to get ContainerCopyObjectInfo from Value".to_string()),
        }
    }
}

pub struct UpdateWorkerInfo {
    pub worker_id: WorkerId,
    pub component_version: ComponentVersion,
    pub update_mode: UpdateMode,
}

impl FromValue for UpdateWorkerInfo {
    fn from_value(value: &Value) -> Result<Self, String>
    where
        Self: Sized,
    {
        match value {
            Value::Record(values) => {
                if values.len() != 3 {
                    Err("Failed to get UpdateWorkerInfo from Value".to_string())
                } else {
                    let worker_id = WorkerId::from_value(&values[0])?;
                    let component_version = u64::from_value(&values[1])?;
                    let update_mode_str = String::from_value(&values[2])?;
                    let update_mode = match update_mode_str.as_str() {
                        "Automatic" => UpdateMode::Automatic,
                        "Manual" => UpdateMode::Manual,
                        _ => return Err("Failed to get UpdateWorkerInfo from Value".to_string()),
                    };

                    Ok(UpdateWorkerInfo {
                        worker_id,
                        component_version,
                        update_mode,
                    })
                }
            }

            _ => Err("Failed to get UpdateWorkerInfo from Value".to_string()),
        }
    }
}

pub struct ContainerObjectLength {
    pub container: String,
    pub object: String,
    pub length: u64,
}

impl FromValue for ContainerObjectLength {
    fn from_value(value: &Value) -> Result<Self, String>
    where
        Self: Sized,
    {
        match value {
            Value::Record(values) => {
                if values.len() != 3 {
                    Err("Failed to get ContainerObjectLength from Value".to_string())
                } else {
                    let container = String::from_value(&values[0])?;
                    let object = String::from_value(&values[1])?;
                    let length = u64::from_value(&values[2])?;

                    Ok(ContainerObjectLength {
                        container,
                        object,
                        length,
                    })
                }
            }

            _ => Err("Failed to get ContainerObjectLength from Value".to_string()),
        }
    }
}

pub struct ContainerAndObjects {
    pub container: String,
    pub objects: Vec<String>,
}

impl FromValue for ContainerAndObjects {
    fn from_value(value: &Value) -> Result<Self, String>
    where
        Self: Sized,
    {
        match value {
            Value::Record(values) => {
                if values.len() != 2 {
                    Err("Failed to get Container from Value".to_string())
                } else {
                    let container = String::from_value(&values[0])?;
                    let objects = Vec::from_value(&values[1])?;

                    Ok(ContainerAndObjects { container, objects })
                }
            }

            _ => Err("Failed to get ContainerAndObjects from Value".to_string()),
        }
    }
}

pub struct Bucket(pub String);

impl FromValue for Bucket {
    fn from_value(value: &Value) -> Result<Self, String>
    where
        Self: Sized,
    {
        match value {
            Value::Record(values) => {
                if values.len() != 1 {
                    Err("Failed to get Bucket from Value".to_string())
                } else {
                    let container_str = String::from_value(&values[0])?;

                    Ok(Bucket(container_str))
                }
            }

            _ => Err("Failed to get Bucket from Value".to_string()),
        }
    }
}

pub struct BucketAndKey {
    pub bucket: String,
    pub key: String,
}

impl FromValue for BucketAndKey {
    fn from_value(value: &Value) -> Result<Self, String>
    where
        Self: Sized,
    {
        match value {
            Value::Record(values) => {
                if values.len() != 2 {
                    Err("Failed to get BucketAndKey from Value".to_string())
                } else {
                    let bucket = String::from_value(&values[0])?;
                    let key = String::from_value(&values[1])?;

                    Ok(BucketAndKey { bucket, key })
                }
            }

            _ => Err("Failed to get BucketAndKey from Value".to_string()),
        }
    }
}

pub struct BucketKeyValue {
    pub bucket: String,
    pub key: String,
    pub value: u64,
}

impl FromValue for BucketKeyValue {
    fn from_value(value: &Value) -> Result<Self, String>
    where
        Self: Sized,
    {
        match value {
            Value::Record(values) => {
                if values.len() != 3 {
                    Err("Failed to get BucketKeyValue from Value".to_string())
                } else {
                    let bucket = String::from_value(&values[0])?;
                    let key = String::from_value(&values[1])?;
                    let value = u64::from_value(&values[1])?;

                    Ok(BucketKeyValue { bucket, key, value })
                }
            }

            _ => Err("Failed to get BucketKeyValue from Value".to_string()),
        }
    }
}

pub struct BucketKeyValues {
    pub bucket: String,
    pub key_values: Vec<(String, u64)>,
}

struct KeyValue {
    key: String,
    value: u64,
}

impl FromValue for KeyValue {
    fn from_value(value: &Value) -> Result<Self, String>
    where
        Self: Sized,
    {
        match value {
            Value::Record(values) => {
                if values.len() != 2 {
                    Err("Failed to get KeyValue from Value".to_string())
                } else {
                    let key = String::from_value(&values[0])?;
                    let value = u64::from_value(&values[1])?;

                    Ok(KeyValue { key, value })
                }
            }

            _ => Err("Failed to get KeyValue from Value".to_string()),
        }
    }
}

impl FromValue for BucketKeyValues {
    fn from_value(value: &Value) -> Result<Self, String>
    where
        Self: Sized,
    {
        match value {
            Value::Record(values) => {
                if values.len() != 2 {
                    Err("Failed to get BucketKeyValues from Value".to_string())
                } else {
                    let bucket = String::from_value(&values[0])?;
                    let key_values: Vec<KeyValue> = Vec::from_value(&values[1])?;

                    Ok(BucketKeyValues {
                        bucket,
                        key_values: key_values.into_iter().map(|x| (x.key, x.value)).collect(),
                    })
                }
            }

            _ => Err("Failed to get BucketKeyValues from Value".to_string()),
        }
    }
}

pub struct BucketAndKeys {
    pub bucket: String,
    pub keys: Vec<String>,
}

impl FromValue for BucketAndKeys {
    fn from_value(value: &Value) -> Result<Self, String>
    where
        Self: Sized,
    {
        match value {
            Value::Record(values) => {
                if values.len() != 2 {
                    Err("Failed to get BucketAndKeys from Value".to_string())
                } else {
                    let bucket = String::from_value(&values[0])?;
                    let keys = Vec::from_value(&values[1])?;

                    Ok(BucketAndKeys { bucket, keys })
                }
            }

            _ => Err("Failed to get BucketAndKeys from Value".to_string()),
        }
    }
}
