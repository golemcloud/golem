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

use crate::error::GolemError;
use crate::services::rdbms::Error as RdbmsError;
use crate::services::rpc::RpcError;
use crate::services::worker_proxy::WorkerProxyError;
use anyhow::anyhow;
use bincode::{Decode, Encode};
use chrono::{DateTime, Timelike, Utc};
use golem_wasm_ast::analysis::{analysed_type, AnalysedType};
use golem_wasm_rpc::{IntoValue, Value};
use golem_wasm_rpc_derive::IntoValue;
use std::fmt::{Display, Formatter};
use std::ops::Add;
use std::time::{Duration, SystemTime};
use wasmtime_wasi::bindings::sockets::ip_name_lookup::IpAddress;
use wasmtime_wasi::bindings::{filesystem, sockets};
use wasmtime_wasi::{FsError, SocketError, StreamError};

#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode, IntoValue)]
pub struct SerializableDateTime {
    pub seconds: u64,
    pub nanoseconds: u32,
}

impl From<wasmtime_wasi::bindings::clocks::wall_clock::Datetime> for SerializableDateTime {
    fn from(value: wasmtime_wasi::bindings::clocks::wall_clock::Datetime) -> Self {
        Self {
            seconds: value.seconds,
            nanoseconds: value.nanoseconds,
        }
    }
}

impl From<SerializableDateTime> for wasmtime_wasi::bindings::clocks::wall_clock::Datetime {
    fn from(value: SerializableDateTime) -> Self {
        Self {
            seconds: value.seconds,
            nanoseconds: value.nanoseconds,
        }
    }
}

impl From<SerializableDateTime> for SystemTime {
    fn from(value: SerializableDateTime) -> Self {
        SystemTime::UNIX_EPOCH.add(Duration::new(value.seconds, value.nanoseconds))
    }
}

impl From<SystemTime> for SerializableDateTime {
    fn from(value: SystemTime) -> Self {
        let duration = value.duration_since(SystemTime::UNIX_EPOCH).unwrap();
        Self {
            seconds: duration.as_secs(),
            nanoseconds: duration.subsec_nanos(),
        }
    }
}

impl From<SerializableDateTime> for cap_std::time::SystemTime {
    fn from(value: SerializableDateTime) -> Self {
        cap_std::time::SystemTime::from_std(value.into())
    }
}

impl From<cap_std::time::SystemTime> for SerializableDateTime {
    fn from(value: cap_std::time::SystemTime) -> Self {
        Self::from(value.into_std())
    }
}

impl From<SerializableDateTime> for DateTime<Utc> {
    fn from(value: SerializableDateTime) -> Self {
        Self::from_timestamp(value.seconds as i64, value.nanoseconds).expect("not a valid datetime")
    }
}

impl From<DateTime<Utc>> for SerializableDateTime {
    fn from(value: DateTime<Utc>) -> Self {
        Self {
            seconds: value.timestamp() as u64,
            nanoseconds: value.nanosecond(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub enum SerializableError {
    Generic { message: String },
    FsError { code: u8 },
    Golem { error: GolemError },
    SocketError { code: u8 },
    Rpc { error: RpcError },
    WorkerProxy { error: WorkerProxyError },
    Rdbms { error: RdbmsError },
}

impl Display for SerializableError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SerializableError::Generic { message } => write!(f, "{}", message),
            SerializableError::FsError { code } => {
                let decoded = decode_fs_error(*code);
                match decoded {
                    Some(error_code) => {
                        write!(f, "File system error: {}", error_code)
                    }
                    None => {
                        write!(f, "File system error: unknown error code {}", code)
                    }
                }
            }
            SerializableError::Golem { error } => write!(f, "{error}"),
            SerializableError::SocketError { code } => {
                let decoded = decode_socket_error(*code);
                match decoded {
                    Some(error_code) => {
                        write!(f, "Socket error: {}", error_code)
                    }
                    None => {
                        write!(f, "Socket error: unknown error code {}", code)
                    }
                }
            }
            SerializableError::Rpc { error } => write!(f, "Rpc error: {error}"),
            SerializableError::WorkerProxy { error } => write!(f, "Worker service error: {error}"),
            SerializableError::Rdbms { error } => write!(f, "RDBMS error: {error}"),
        }
    }
}

// We simply encode SerializableErrors for the public oplog as string. This simplified
// significantly the size of each ValueAndType oplog entry and it is also necessary
// because currently AnalysedType does not support recursion and GolemError is recursive.
impl IntoValue for SerializableError {
    fn into_value(self) -> Value {
        Value::String(self.to_string())
    }

    fn get_type() -> AnalysedType {
        analysed_type::str()
    }
}

fn get_fs_error_code(value: &filesystem::types::ErrorCode) -> u8 {
    match value {
        filesystem::types::ErrorCode::Access => 0,
        filesystem::types::ErrorCode::WouldBlock => 1,
        filesystem::types::ErrorCode::Already => 2,
        filesystem::types::ErrorCode::BadDescriptor => 3,
        filesystem::types::ErrorCode::Busy => 4,
        filesystem::types::ErrorCode::Deadlock => 5,
        filesystem::types::ErrorCode::Quota => 6,
        filesystem::types::ErrorCode::Exist => 7,
        filesystem::types::ErrorCode::FileTooLarge => 8,
        filesystem::types::ErrorCode::IllegalByteSequence => 9,
        filesystem::types::ErrorCode::InProgress => 10,
        filesystem::types::ErrorCode::Interrupted => 11,
        filesystem::types::ErrorCode::Invalid => 12,
        filesystem::types::ErrorCode::Io => 13,
        filesystem::types::ErrorCode::IsDirectory => 14,
        filesystem::types::ErrorCode::Loop => 15,
        filesystem::types::ErrorCode::TooManyLinks => 16,
        filesystem::types::ErrorCode::MessageSize => 17,
        filesystem::types::ErrorCode::NameTooLong => 18,
        filesystem::types::ErrorCode::NoDevice => 19,
        filesystem::types::ErrorCode::NoEntry => 20,
        filesystem::types::ErrorCode::NoLock => 21,
        filesystem::types::ErrorCode::InsufficientMemory => 22,
        filesystem::types::ErrorCode::InsufficientSpace => 23,
        filesystem::types::ErrorCode::NotDirectory => 24,
        filesystem::types::ErrorCode::NotEmpty => 25,
        filesystem::types::ErrorCode::NotRecoverable => 26,
        filesystem::types::ErrorCode::Unsupported => 27,
        filesystem::types::ErrorCode::NoTty => 28,
        filesystem::types::ErrorCode::NoSuchDevice => 29,
        filesystem::types::ErrorCode::Overflow => 30,
        filesystem::types::ErrorCode::NotPermitted => 31,
        filesystem::types::ErrorCode::Pipe => 32,
        filesystem::types::ErrorCode::ReadOnly => 33,
        filesystem::types::ErrorCode::InvalidSeek => 34,
        filesystem::types::ErrorCode::TextFileBusy => 35,
        filesystem::types::ErrorCode::CrossDevice => 36,
    }
}

fn decode_fs_error(code: u8) -> Option<filesystem::types::ErrorCode> {
    match code {
        0 => Some(filesystem::types::ErrorCode::Access),
        1 => Some(filesystem::types::ErrorCode::WouldBlock),
        2 => Some(filesystem::types::ErrorCode::Already),
        3 => Some(filesystem::types::ErrorCode::BadDescriptor),
        4 => Some(filesystem::types::ErrorCode::Busy),
        5 => Some(filesystem::types::ErrorCode::Deadlock),
        6 => Some(filesystem::types::ErrorCode::Quota),
        7 => Some(filesystem::types::ErrorCode::Exist),
        8 => Some(filesystem::types::ErrorCode::FileTooLarge),
        9 => Some(filesystem::types::ErrorCode::IllegalByteSequence),
        10 => Some(filesystem::types::ErrorCode::InProgress),
        11 => Some(filesystem::types::ErrorCode::Interrupted),
        12 => Some(filesystem::types::ErrorCode::Invalid),
        13 => Some(filesystem::types::ErrorCode::Io),
        14 => Some(filesystem::types::ErrorCode::IsDirectory),
        15 => Some(filesystem::types::ErrorCode::Loop),
        16 => Some(filesystem::types::ErrorCode::TooManyLinks),
        17 => Some(filesystem::types::ErrorCode::MessageSize),
        18 => Some(filesystem::types::ErrorCode::NameTooLong),
        19 => Some(filesystem::types::ErrorCode::NoDevice),
        20 => Some(filesystem::types::ErrorCode::NoEntry),
        21 => Some(filesystem::types::ErrorCode::NoLock),
        22 => Some(filesystem::types::ErrorCode::InsufficientMemory),
        23 => Some(filesystem::types::ErrorCode::InsufficientSpace),
        24 => Some(filesystem::types::ErrorCode::NotDirectory),
        25 => Some(filesystem::types::ErrorCode::NotEmpty),
        26 => Some(filesystem::types::ErrorCode::NotRecoverable),
        27 => Some(filesystem::types::ErrorCode::Unsupported),
        28 => Some(filesystem::types::ErrorCode::NoTty),
        29 => Some(filesystem::types::ErrorCode::NoSuchDevice),
        30 => Some(filesystem::types::ErrorCode::Overflow),
        31 => Some(filesystem::types::ErrorCode::NotPermitted),
        32 => Some(filesystem::types::ErrorCode::Pipe),
        33 => Some(filesystem::types::ErrorCode::ReadOnly),
        34 => Some(filesystem::types::ErrorCode::InvalidSeek),
        35 => Some(filesystem::types::ErrorCode::TextFileBusy),
        36 => Some(filesystem::types::ErrorCode::CrossDevice),
        _ => None,
    }
}

fn get_socket_error_code(code: &sockets::network::ErrorCode) -> u8 {
    match code {
        sockets::network::ErrorCode::Unknown => 0,
        sockets::network::ErrorCode::AccessDenied => 1,
        sockets::network::ErrorCode::NotSupported => 2,
        sockets::network::ErrorCode::InvalidArgument => 3,
        sockets::network::ErrorCode::OutOfMemory => 4,
        sockets::network::ErrorCode::Timeout => 5,
        sockets::network::ErrorCode::ConcurrencyConflict => 6,
        sockets::network::ErrorCode::NotInProgress => 7,
        sockets::network::ErrorCode::WouldBlock => 8,
        sockets::network::ErrorCode::InvalidState => 9,
        sockets::network::ErrorCode::NewSocketLimit => 10,
        sockets::network::ErrorCode::AddressNotBindable => 11,
        sockets::network::ErrorCode::AddressInUse => 12,
        sockets::network::ErrorCode::RemoteUnreachable => 13,
        sockets::network::ErrorCode::ConnectionRefused => 14,
        sockets::network::ErrorCode::ConnectionReset => 15,
        sockets::network::ErrorCode::ConnectionAborted => 16,
        sockets::network::ErrorCode::DatagramTooLarge => 17,
        sockets::network::ErrorCode::NameUnresolvable => 18,
        sockets::network::ErrorCode::TemporaryResolverFailure => 19,
        sockets::network::ErrorCode::PermanentResolverFailure => 20,
    }
}

fn decode_socket_error(code: u8) -> Option<sockets::network::ErrorCode> {
    match code {
        0 => Some(sockets::network::ErrorCode::Unknown),
        1 => Some(sockets::network::ErrorCode::AccessDenied),
        2 => Some(sockets::network::ErrorCode::NotSupported),
        3 => Some(sockets::network::ErrorCode::InvalidArgument),
        4 => Some(sockets::network::ErrorCode::OutOfMemory),
        5 => Some(sockets::network::ErrorCode::Timeout),
        6 => Some(sockets::network::ErrorCode::ConcurrencyConflict),
        7 => Some(sockets::network::ErrorCode::NotInProgress),
        8 => Some(sockets::network::ErrorCode::WouldBlock),
        9 => Some(sockets::network::ErrorCode::InvalidState),
        10 => Some(sockets::network::ErrorCode::NewSocketLimit),
        11 => Some(sockets::network::ErrorCode::AddressNotBindable),
        12 => Some(sockets::network::ErrorCode::AddressInUse),
        13 => Some(sockets::network::ErrorCode::RemoteUnreachable),
        14 => Some(sockets::network::ErrorCode::ConnectionRefused),
        15 => Some(sockets::network::ErrorCode::ConnectionReset),
        16 => Some(sockets::network::ErrorCode::ConnectionAborted),
        17 => Some(sockets::network::ErrorCode::DatagramTooLarge),
        18 => Some(sockets::network::ErrorCode::NameUnresolvable),
        19 => Some(sockets::network::ErrorCode::TemporaryResolverFailure),
        20 => Some(sockets::network::ErrorCode::PermanentResolverFailure),
        _ => None,
    }
}

impl From<&anyhow::Error> for SerializableError {
    fn from(value: &anyhow::Error) -> Self {
        Self::Generic {
            message: value.to_string(),
        }
    }
}

impl From<FsError> for SerializableError {
    fn from(value: FsError) -> Self {
        Self::from(&value)
    }
}

impl From<&FsError> for SerializableError {
    fn from(value: &FsError) -> Self {
        let code = value.downcast_ref();
        match code {
            Some(code) => Self::FsError {
                code: get_fs_error_code(code),
            },
            None => Self::Generic {
                message: value.to_string(),
            },
        }
    }
}

impl From<SerializableError> for anyhow::Error {
    fn from(value: SerializableError) -> Self {
        match value {
            SerializableError::Generic { message } => anyhow::Error::msg(message),
            SerializableError::FsError { code } => {
                let error_code = decode_fs_error(code);
                match error_code {
                    Some(code) => anyhow!(FsError::from(code)),
                    None => anyhow::Error::msg(format!("Unknown file-system error code: {}", code)),
                }
            }
            SerializableError::Golem { error } => anyhow!(error),
            SerializableError::SocketError { code } => {
                let error_code = decode_socket_error(code);
                match error_code {
                    Some(code) => anyhow!(SocketError::from(code)),
                    None => anyhow::Error::msg(format!("Unknown socket error code: {}", code)),
                }
            }
            SerializableError::Rpc { error } => anyhow!(error),
            SerializableError::WorkerProxy { error } => anyhow!(error),
            SerializableError::Rdbms { error } => anyhow!(error),
        }
    }
}

impl From<SerializableError> for FsError {
    fn from(value: SerializableError) -> Self {
        match value {
            SerializableError::Generic { message } => FsError::trap(anyhow::Error::msg(message)),
            SerializableError::FsError { code } => {
                let error_code = decode_fs_error(code);
                match error_code {
                    Some(code) => FsError::from(code),
                    None => FsError::trap(anyhow::Error::msg(format!(
                        "Unknown file-system error code: {}",
                        code
                    ))),
                }
            }
            SerializableError::Golem { error } => FsError::trap(anyhow!(error)),
            SerializableError::SocketError { .. } => {
                let anyhow: anyhow::Error = value.into();
                FsError::trap(anyhow)
            }
            SerializableError::Rpc { error } => FsError::trap(anyhow!(error)),
            SerializableError::WorkerProxy { error } => FsError::trap(anyhow!(error)),
            SerializableError::Rdbms { error } => FsError::trap(anyhow!(error)),
        }
    }
}

impl From<GolemError> for SerializableError {
    fn from(value: GolemError) -> Self {
        Self::Golem { error: value }
    }
}

impl From<&GolemError> for SerializableError {
    fn from(value: &GolemError) -> Self {
        Self::Golem {
            error: value.clone(),
        }
    }
}

impl From<SerializableError> for GolemError {
    fn from(value: SerializableError) -> Self {
        match value {
            SerializableError::Generic { message } => GolemError::unknown(message),
            SerializableError::FsError { .. } => {
                let anyhow: anyhow::Error = value.into();
                GolemError::unknown(anyhow.to_string())
            }
            SerializableError::Golem { error } => error,
            SerializableError::SocketError { .. } => {
                let anyhow: anyhow::Error = value.into();
                GolemError::unknown(anyhow.to_string())
            }
            SerializableError::Rpc { error } => GolemError::unknown(error.to_string()),
            SerializableError::WorkerProxy { error } => GolemError::unknown(error.to_string()),
            SerializableError::Rdbms { error } => GolemError::unknown(error.to_string()),
        }
    }
}

impl From<&SocketError> for SerializableError {
    fn from(value: &SocketError) -> Self {
        let code = value.downcast_ref();
        match code {
            Some(code) => Self::SocketError {
                code: get_socket_error_code(code),
            },
            None => Self::Generic {
                message: value.to_string(),
            },
        }
    }
}

impl From<SerializableError> for SocketError {
    fn from(value: SerializableError) -> Self {
        match value {
            SerializableError::Generic { message } => {
                SocketError::trap(anyhow::Error::msg(message))
            }
            SerializableError::FsError { .. } => {
                let anyhow: anyhow::Error = value.into();
                SocketError::trap(anyhow)
            }
            SerializableError::Golem { error } => SocketError::trap(anyhow!(error)),
            SerializableError::SocketError { code } => {
                let error_code = decode_socket_error(code);
                match error_code {
                    Some(code) => SocketError::from(code),
                    None => SocketError::trap(anyhow::Error::msg(format!(
                        "Unknown file-system error code: {}",
                        code
                    ))),
                }
            }
            SerializableError::Rpc { error } => SocketError::trap(anyhow!(error)),
            SerializableError::WorkerProxy { error } => SocketError::trap(anyhow!(error)),
            SerializableError::Rdbms { error } => SocketError::trap(anyhow!(error)),
        }
    }
}

impl From<&RpcError> for SerializableError {
    fn from(value: &RpcError) -> Self {
        Self::Rpc {
            error: value.clone(),
        }
    }
}

impl From<SerializableError> for RpcError {
    fn from(value: SerializableError) -> Self {
        match value {
            SerializableError::Generic { message } => RpcError::ProtocolError { details: message },
            SerializableError::FsError { .. } => {
                let anyhow: anyhow::Error = value.into();
                RpcError::ProtocolError {
                    details: anyhow.to_string(),
                }
            }
            SerializableError::Golem { error } => RpcError::ProtocolError {
                details: error.to_string(),
            },
            SerializableError::SocketError { .. } => {
                let anyhow: anyhow::Error = value.into();
                RpcError::ProtocolError {
                    details: anyhow.to_string(),
                }
            }
            SerializableError::Rpc { error } => error,
            SerializableError::WorkerProxy { error } => RpcError::ProtocolError {
                details: error.to_string(),
            },
            SerializableError::Rdbms { error } => RpcError::ProtocolError {
                details: error.to_string(),
            },
        }
    }
}

impl From<&WorkerProxyError> for SerializableError {
    fn from(value: &WorkerProxyError) -> Self {
        Self::WorkerProxy {
            error: value.clone(),
        }
    }
}

impl From<SerializableError> for WorkerProxyError {
    fn from(value: SerializableError) -> Self {
        match value {
            SerializableError::Generic { message } => {
                WorkerProxyError::InternalError(GolemError::unknown(message))
            }
            SerializableError::FsError { .. } => {
                let anyhow: anyhow::Error = value.into();
                WorkerProxyError::InternalError(GolemError::unknown(anyhow.to_string()))
            }
            SerializableError::Golem { error } => WorkerProxyError::InternalError(error),
            SerializableError::SocketError { .. } => {
                let anyhow: anyhow::Error = value.into();
                WorkerProxyError::InternalError(GolemError::unknown(anyhow.to_string()))
            }
            SerializableError::Rpc { .. } => {
                let anyhow: anyhow::Error = value.into();
                WorkerProxyError::InternalError(GolemError::unknown(anyhow.to_string()))
            }
            SerializableError::WorkerProxy { error } => error,
            SerializableError::Rdbms { .. } => {
                let anyhow: anyhow::Error = value.into();
                WorkerProxyError::InternalError(GolemError::unknown(anyhow.to_string()))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode, IntoValue)]
pub enum SerializableStreamError {
    Closed,
    LastOperationFailed(SerializableError),
    Trap(SerializableError),
}

impl From<&StreamError> for SerializableStreamError {
    fn from(value: &StreamError) -> Self {
        match value {
            StreamError::Closed => Self::Closed,
            StreamError::LastOperationFailed(e) => Self::LastOperationFailed(e.into()),
            StreamError::Trap(e) => Self::Trap(e.into()),
        }
    }
}

impl From<SerializableStreamError> for StreamError {
    fn from(value: SerializableStreamError) -> Self {
        match value {
            SerializableStreamError::Closed => Self::Closed,
            SerializableStreamError::LastOperationFailed(e) => Self::LastOperationFailed(e.into()),
            SerializableStreamError::Trap(e) => Self::Trap(e.into()),
        }
    }
}

impl From<GolemError> for SerializableStreamError {
    fn from(value: GolemError) -> Self {
        Self::Trap(value.into())
    }
}

impl From<&RdbmsError> for SerializableError {
    fn from(value: &RdbmsError) -> Self {
        SerializableError::Rdbms {
            error: value.clone(),
        }
    }
}

impl From<SerializableError> for RdbmsError {
    fn from(value: SerializableError) -> Self {
        match value {
            SerializableError::Generic { message } => RdbmsError::other_response_failure(message),
            SerializableError::FsError { .. } => {
                let anyhow: anyhow::Error = value.into();
                RdbmsError::other_response_failure(anyhow)
            }
            SerializableError::Golem { error } => RdbmsError::other_response_failure(error),
            SerializableError::SocketError { .. } => {
                let anyhow: anyhow::Error = value.into();
                RdbmsError::other_response_failure(anyhow)
            }
            SerializableError::Rpc { error } => RdbmsError::other_response_failure(error),
            SerializableError::WorkerProxy { error } => RdbmsError::other_response_failure(error),
            SerializableError::Rdbms { error } => error,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub enum SerializableIpAddress {
    IPv4 { address: [u8; 4] },
    IPv6 { address: [u16; 8] },
}

impl From<IpAddress> for SerializableIpAddress {
    fn from(value: IpAddress) -> Self {
        match value {
            IpAddress::Ipv4(address) => SerializableIpAddress::IPv4 {
                address: [address.0, address.1, address.2, address.3],
            },
            IpAddress::Ipv6(address) => SerializableIpAddress::IPv6 {
                address: [
                    address.0, address.1, address.2, address.3, address.4, address.5, address.6,
                    address.7,
                ],
            },
        }
    }
}

impl From<SerializableIpAddress> for IpAddress {
    fn from(value: SerializableIpAddress) -> Self {
        match value {
            SerializableIpAddress::IPv4 { address } => {
                IpAddress::Ipv4((address[0], address[1], address[2], address[3]))
            }
            SerializableIpAddress::IPv6 { address } => IpAddress::Ipv6((
                address[0], address[1], address[2], address[3], address[4], address[5], address[6],
                address[7],
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode, IntoValue)]
pub struct SerializableIpAddresses(pub Vec<SerializableIpAddress>);

impl From<Vec<IpAddress>> for SerializableIpAddresses {
    fn from(value: Vec<IpAddress>) -> Self {
        SerializableIpAddresses(value.into_iter().map(|v| v.into()).collect())
    }
}

impl From<SerializableIpAddresses> for Vec<IpAddress> {
    fn from(value: SerializableIpAddresses) -> Self {
        value.0.into_iter().map(|v| v.into()).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode, IntoValue)]
pub struct SerializableFileTimes {
    pub data_access_timestamp: Option<SerializableDateTime>,
    pub data_modification_timestamp: Option<SerializableDateTime>,
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::durable_host::serialized::{
        SerializableDateTime, SerializableError, SerializableIpAddress, SerializableIpAddresses,
        SerializableStreamError,
    };
    use crate::error::GolemError;
    use crate::model::InterruptKind;
    use golem_common::model::oplog::OplogIndex;
    use golem_common::model::{ComponentId, PromiseId, ShardId, WorkerId};
    use proptest::collection::vec;
    use proptest::prelude::*;
    use proptest::strategy::LazyJust;
    use std::ops::Add;
    use std::time::{Duration, SystemTime};
    use uuid::Uuid;
    use wasmtime_wasi::bindings::sockets::network::IpAddress;
    use wasmtime_wasi::bindings::{filesystem, sockets};
    use wasmtime_wasi::{FsError, SocketError, StreamError};

    fn datetime_strat(
    ) -> impl Strategy<Value = wasmtime_wasi::bindings::clocks::wall_clock::Datetime> {
        (0..(u64::MAX / 1_000_000_000), 0..999_999_999u32).prop_map(|(seconds, nanoseconds)| {
            wasmtime_wasi::bindings::clocks::wall_clock::Datetime {
                seconds,
                nanoseconds,
            }
        })
    }

    fn systemtime_strat() -> impl Strategy<Value = SystemTime> {
        (0..(u64::MAX / 1_000_000_000), 0..999_999_999u32).prop_map(|(seconds, nanoseconds)| {
            SystemTime::UNIX_EPOCH.add(Duration::new(seconds, nanoseconds))
        })
    }

    fn cap_std_systemtime_strat() -> impl Strategy<Value = cap_std::time::SystemTime> {
        (0..(u64::MAX / 1_000_000_000), 0..999_999_999u32).prop_map(|(seconds, nanoseconds)| {
            cap_std::time::SystemTime::from_std(
                SystemTime::UNIX_EPOCH.add(Duration::new(seconds, nanoseconds)),
            )
        })
    }

    fn anyhow_strat() -> impl Strategy<Value = anyhow::Error> {
        ".*".prop_map(anyhow::Error::msg)
    }

    fn fserror_strat() -> impl Strategy<Value = FsError> {
        prop_oneof! {
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Access)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::WouldBlock)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Already)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::BadDescriptor)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Busy)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Deadlock)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Quota)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Exist)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::FileTooLarge)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::IllegalByteSequence)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::InProgress)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Interrupted)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Invalid)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Io)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::IsDirectory)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Loop)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::TooManyLinks)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::MessageSize)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NameTooLong)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NoDevice)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NoEntry)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NoLock)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::InsufficientMemory)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::InsufficientSpace)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NotDirectory)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NotEmpty)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NotRecoverable)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Unsupported)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NoTty)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NoSuchDevice)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Overflow)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::NotPermitted)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::Pipe)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::ReadOnly)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::InvalidSeek)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::TextFileBusy)),
            LazyJust::new(|| FsError::from(filesystem::types::ErrorCode::CrossDevice)),
            anyhow_strat().prop_map(FsError::trap),
        }
    }

    fn uuid_strat() -> impl Strategy<Value = Uuid> {
        (any::<u64>(), any::<u64>()).prop_map(|(a, b)| Uuid::from_u64_pair(a, b))
    }

    fn componentid_strat() -> impl Strategy<Value = ComponentId> {
        uuid_strat().prop_map(ComponentId)
    }

    fn workerid_strat() -> impl Strategy<Value = WorkerId> {
        (componentid_strat(), ".+").prop_map(|(component_id, worker_name)| WorkerId {
            component_id,
            worker_name,
        })
    }

    fn promiseid_strat() -> impl Strategy<Value = PromiseId> {
        (workerid_strat(), any::<u64>()).prop_map(|(worker_id, oplog_idx)| PromiseId {
            worker_id,
            oplog_idx: OplogIndex::from_u64(oplog_idx),
        })
    }

    fn interrupt_kind_strat() -> impl Strategy<Value = InterruptKind> {
        prop_oneof! {
            Just(InterruptKind::Interrupt),
            Just(InterruptKind::Restart),
            Just(InterruptKind::Suspend),
        }
    }

    fn shardid_strat() -> impl Strategy<Value = ShardId> {
        any::<i64>().prop_map(ShardId::new)
    }

    fn golemerror_strat() -> impl Strategy<Value = GolemError> {
        prop_oneof! {
            ".*".prop_map(|details| GolemError::InvalidRequest { details }),
            workerid_strat().prop_map(|worker_id| GolemError::WorkerAlreadyExists { worker_id }),
            workerid_strat().prop_map(|worker_id| GolemError::WorkerNotFound { worker_id }),
            (workerid_strat(), ".*").prop_map(|(worker_id, details)| GolemError::WorkerCreationFailed { worker_id, details }),
            (workerid_strat(), ".*").prop_map(|(worker_id, reason)| GolemError::FailedToResumeWorker { worker_id, reason: Box::new(GolemError::unknown(reason)) }),
            (componentid_strat(), any::<u64>(), ".*").prop_map(|(component_id, component_version, reason)| GolemError::ComponentDownloadFailed { component_id, component_version, reason }),
            (componentid_strat(), any::<u64>(), ".*").prop_map(|(component_id, component_version, reason)| GolemError::ComponentParseFailed { component_id, component_version, reason }),
            (componentid_strat(), ".*").prop_map(|(component_id, reason)| GolemError::GetLatestVersionOfComponentFailed { component_id, reason }),
            promiseid_strat().prop_map(|promise_id| GolemError::PromiseNotFound { promise_id }),
            promiseid_strat().prop_map(|promise_id| GolemError::PromiseDropped { promise_id }),
            promiseid_strat().prop_map(|promise_id| GolemError::PromiseAlreadyCompleted { promise_id }),
            promiseid_strat().prop_map(|promise_id| GolemError::PromiseAlreadyCompleted { promise_id }),
            interrupt_kind_strat().prop_map(|kind| GolemError::Interrupted { kind }),
            ".*".prop_map(|details| GolemError::ParamTypeMismatch { details }),
            Just(GolemError::NoValueInMessage),
            ".*".prop_map(|details| GolemError::ValueMismatch { details }),
            (".*", ".*").prop_map(|(expected, got)| GolemError::UnexpectedOplogEntry { expected, got }),
            ".*".prop_map(|details| GolemError::Runtime { details }),
            (shardid_strat(), vec(shardid_strat(), 0..100)).prop_map(|(shard_id, shard_ids)| GolemError::InvalidShardId { shard_id, shard_ids }),
            Just(GolemError::InvalidAccount),
            ".*".prop_map(|details| GolemError::PreviousInvocationFailed { details }),
            Just(GolemError::PreviousInvocationExited),
            ".*".prop_map(|details| GolemError::Unknown { details }),
            (".*", ".*").prop_map(|(path, reason)| GolemError::InitialComponentFileDownloadFailed { path, reason }),
        }
    }

    fn socketerror_strat() -> impl Strategy<Value = SocketError> {
        prop_oneof! {
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::Unknown)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::AccessDenied)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::NotSupported)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::InvalidArgument)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::OutOfMemory)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::Timeout)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::ConcurrencyConflict)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::NotInProgress)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::WouldBlock)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::InvalidState)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::NewSocketLimit)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::AddressNotBindable)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::AddressInUse)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::RemoteUnreachable)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::ConnectionRefused)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::ConnectionReset)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::ConnectionAborted)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::DatagramTooLarge)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::NameUnresolvable)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::TemporaryResolverFailure)),
            LazyJust::new(|| SocketError::from(sockets::network::ErrorCode::PermanentResolverFailure)),
            anyhow_strat().prop_map(SocketError::trap),
        }
    }

    fn streamerror_strat() -> impl Strategy<Value = StreamError> {
        prop_oneof! {
            LazyJust::new(|| StreamError::Closed),
            anyhow_strat().prop_map(StreamError::LastOperationFailed),
            anyhow_strat().prop_map(StreamError::Trap)
        }
    }

    fn ipaddress_strat() -> impl Strategy<Value = IpAddress> {
        prop_oneof! {
            (any::<u8>(), any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(a, b, c, d)| IpAddress::Ipv4((a, b, c, d))),
            (any::<u16>(), any::<u16>(), any::<u16>(), any::<u16>(), any::<u16>(), any::<u16>(), any::<u16>(), any::<u16>()).prop_map(|(a, b, c, d, e, f, g, h)| IpAddress::Ipv6((a, b, c, d, e, f, g, h))),
        }
    }

    proptest! {
        #[test]
        fn roundtrip_wall_clock_datetime(value in datetime_strat()) {
            let serialized: SerializableDateTime = value.into();
            let result: wasmtime_wasi::bindings::clocks::wall_clock::Datetime = serialized.into();
            prop_assert_eq!(value.seconds, result.seconds);
            prop_assert_eq!(value.nanoseconds, result.nanoseconds);
        }

        #[test]
        fn roundtrip_systemtime(value in systemtime_strat()) {
            let serialized: SerializableDateTime = value.into();
            let result: SystemTime = serialized.into();
            prop_assert_eq!(value, result);
        }

        #[test]
        fn roundtrip_cap_std_systemtime(value in cap_std_systemtime_strat()) {
            let serialized: SerializableDateTime = value.into();
            let result: cap_std::time::SystemTime = serialized.into();
            prop_assert_eq!(value, result);
        }

        #[test]
        fn roundtrip_anyhow_error(value in anyhow_strat()) {
            let serialized: SerializableError = (&value).into();
            let result: anyhow::Error = serialized.into();
            prop_assert_eq!(value.to_string(), result.to_string());
        }

        #[test]
        fn roundtrip_fserror(value in fserror_strat()) {
            let serialized: SerializableError = (&value).into();
            let result: FsError = serialized.into();
            let downcasted_value = value.downcast();
            let downcasted_result = result.downcast();

            match (downcasted_value, downcasted_result) {
                (Ok(value), Ok(result)) => prop_assert_eq!(value, result),
                (Err(value), Err(result)) => prop_assert_eq!(value.to_string(), result.to_string()),
                _ => prop_assert!(false),
            }
        }

        #[test]
        fn roundtrip_golemerror(value in golemerror_strat()) {
            let serialized: SerializableError = value.clone().into();
            let result: GolemError = serialized.into();
            prop_assert_eq!(value, result);
        }

        #[test]
        fn roundtrip_socketerror(value in socketerror_strat()) {
            let serialized: SerializableError = (&value).into();
            let result: SocketError = serialized.into();
            let downcasted_value = value.downcast();
            let downcasted_result = result.downcast();

            match (downcasted_value, downcasted_result) {
                (Ok(value), Ok(result)) => prop_assert_eq!(value, result),
                (Err(value), Err(result)) => prop_assert_eq!(value.to_string(), result.to_string()),
                _ => prop_assert!(false),
            }
        }

        #[test]
        fn roundtrip_streamerror(value in streamerror_strat()) {
            let serialized: SerializableStreamError = (&value).into();
            let result: StreamError = serialized.into();

            match (value, result) {
                (StreamError::Closed, StreamError::Closed) => (),
                (StreamError::LastOperationFailed(value), StreamError::LastOperationFailed(result)) => {
                    prop_assert_eq!(value.to_string(), result.to_string());
                },
                (StreamError::Trap(value), StreamError::Trap(result)) => {
                    prop_assert_eq!(value.to_string(), result.to_string());
                },
                _ => prop_assert!(false),
            }
        }

        #[test]
        fn roundtrip_ipaddress(value in ipaddress_strat()) {
            let serialized: SerializableIpAddress = value.into();
            let result: IpAddress = serialized.into();

            match (value, result) {
                (IpAddress::Ipv4(value), IpAddress::Ipv4(result)) => {
                    prop_assert_eq!(value, result);
                },
                (IpAddress::Ipv6(value), IpAddress::Ipv6(result)) => {
                    prop_assert_eq!(value, result);
                },
                _ => prop_assert!(false),
            }
        }

        #[test]
        fn roundtrip_ipaddresses(value in vec(ipaddress_strat(), 0..100)) {
            let serialized: SerializableIpAddresses = value.clone().into();
            let result: Vec<IpAddress> = serialized.into();

            for (value, result) in value.into_iter().zip(result.into_iter()) {
                match (value, result) {
                    (IpAddress::Ipv4(value), IpAddress::Ipv4(result)) => {
                        prop_assert_eq!(value, result);
                    },
                    (IpAddress::Ipv6(value), IpAddress::Ipv6(result)) => {
                        prop_assert_eq!(value, result);
                    },
                    _ => prop_assert!(false),
                }
            }
        }
    }
}
