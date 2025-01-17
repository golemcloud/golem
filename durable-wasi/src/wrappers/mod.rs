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

use crate::bindings;
use crate::bindings::exports::wasi::clocks::wall_clock;
use crate::bindings::exports::wasi::sockets::ip_name_lookup::{ErrorCode, IpAddress};
use bincode::{Decode, Encode};
use golem_common::base_model::{ComponentId, PromiseId, ShardId, WorkerId};
use std::fmt::{Display, Formatter};

mod cli;
mod clock;
mod filesystem;
mod io;
mod logging;
mod random;
mod sockets;
mod http;

// TODO: try to avoid having copies of these types here
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub enum RpcError {
    ProtocolError { details: String },
    Denied { details: String },
    NotFound { details: String },
    RemoteInternalError { details: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub enum WorkerProxyError {
    BadRequest(Vec<String>),
    Unauthorized(String),
    LimitExceeded(String),
    NotFound(String),
    AlreadyExists(String),
    InternalError(GolemError),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub enum GolemError {
    InvalidRequest {
        details: String,
    },
    WorkerAlreadyExists {
        worker_id: WorkerId,
    },
    WorkerNotFound {
        worker_id: WorkerId,
    },
    WorkerCreationFailed {
        worker_id: WorkerId,
        details: String,
    },
    FailedToResumeWorker {
        worker_id: WorkerId,
        reason: Box<GolemError>,
    },
    ComponentDownloadFailed {
        component_id: ComponentId,
        component_version: u64,
        reason: String,
    },
    ComponentParseFailed {
        component_id: ComponentId,
        component_version: u64,
        reason: String,
    },
    GetLatestVersionOfComponentFailed {
        component_id: ComponentId,
        reason: String,
    },
    PromiseNotFound {
        promise_id: PromiseId,
    },
    PromiseDropped {
        promise_id: PromiseId,
    },
    PromiseAlreadyCompleted {
        promise_id: PromiseId,
    },
    Interrupted {
        kind: InterruptKind,
    },
    ParamTypeMismatch {
        details: String,
    },
    NoValueInMessage,
    ValueMismatch {
        details: String,
    },
    UnexpectedOplogEntry {
        expected: String,
        got: String,
    },
    Runtime {
        details: String,
    },
    InvalidShardId {
        shard_id: ShardId,
        shard_ids: Vec<ShardId>,
    },
    InvalidAccount,
    PreviousInvocationFailed {
        details: String,
    },
    PreviousInvocationExited,
    Unknown {
        details: String,
    },
    ShardingNotReady,
    InitialComponentFileDownloadFailed {
        path: String,
        reason: String,
    },
    FileSystemError {
        path: String,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialOrd, PartialEq, Eq, Hash, Encode, Decode)]
pub enum InterruptKind {
    Interrupt,
    Restart,
    Suspend,
    Jump,
}

// Guest binding version of golem-worker-executor-base's SerializableError
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub enum SerializableError {
    Generic { message: String },
    FsError { code: u8 },
    Golem { error: GolemError },
    SocketError { code: u8 },
    Rpc { error: RpcError },
    WorkerProxy { error: WorkerProxyError },
}

// TODO: real implementation
impl Display for SerializableError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SerializableError::Generic { message } => write!(f, "{}", message),
            SerializableError::FsError { code } => write!(f, "FsError({})", code),
            SerializableError::Golem { error } => write!(f, "Golem({:?})", error),
            SerializableError::SocketError { code } => write!(f, "SocketError({})", code),
            SerializableError::Rpc { error } => write!(f, "Rpc({:?})", error),
            SerializableError::WorkerProxy { error } => write!(f, "WorkerProxy({:?})", error),
        }
    }
}

impl From<&ErrorCode> for SerializableError {
    fn from(value: &ErrorCode) -> Self {
        SerializableError::SocketError { code: *value as u8 }
    }
}

impl From<SerializableError> for ErrorCode {
    fn from(value: SerializableError) -> Self {
        match value {
            SerializableError::SocketError { code } => match decode_socket_error(code) {
                Some(error) => error,
                None => panic!("Persisted socket error: {value}"),
            },
            _ => panic!("Persisted socket error: {value}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub struct SerializableDateTime {
    pub seconds: u64,
    pub nanoseconds: u32,
}

impl From<wall_clock::Datetime> for SerializableDateTime {
    fn from(value: wall_clock::Datetime) -> Self {
        Self {
            seconds: value.seconds,
            nanoseconds: value.nanoseconds,
        }
    }
}

impl From<SerializableDateTime> for wall_clock::Datetime {
    fn from(value: SerializableDateTime) -> Self {
        Self {
            seconds: value.seconds,
            nanoseconds: value.nanoseconds,
        }
    }
}

impl From<bindings::wasi::clocks::wall_clock::Datetime> for SerializableDateTime {
    fn from(value: bindings::wasi::clocks::wall_clock::Datetime) -> Self {
        Self {
            seconds: value.seconds,
            nanoseconds: value.nanoseconds,
        }
    }
}

impl From<SerializableDateTime> for bindings::wasi::clocks::wall_clock::Datetime {
    fn from(value: SerializableDateTime) -> Self {
        Self {
            seconds: value.seconds,
            nanoseconds: value.nanoseconds,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub struct SerializableFileTimes {
    pub data_access_timestamp: Option<SerializableDateTime>,
    pub data_modification_timestamp: Option<SerializableDateTime>,
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

#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
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

fn decode_socket_error(code: u8) -> Option<bindings::exports::wasi::sockets::network::ErrorCode> {
    match code {
        0 => Some(bindings::exports::wasi::sockets::network::ErrorCode::Unknown),
        1 => Some(bindings::exports::wasi::sockets::network::ErrorCode::AccessDenied),
        2 => Some(bindings::exports::wasi::sockets::network::ErrorCode::NotSupported),
        3 => Some(bindings::exports::wasi::sockets::network::ErrorCode::InvalidArgument),
        4 => Some(bindings::exports::wasi::sockets::network::ErrorCode::OutOfMemory),
        5 => Some(bindings::exports::wasi::sockets::network::ErrorCode::Timeout),
        6 => Some(bindings::exports::wasi::sockets::network::ErrorCode::ConcurrencyConflict),
        7 => Some(bindings::exports::wasi::sockets::network::ErrorCode::NotInProgress),
        8 => Some(bindings::exports::wasi::sockets::network::ErrorCode::WouldBlock),
        9 => Some(bindings::exports::wasi::sockets::network::ErrorCode::InvalidState),
        10 => Some(bindings::exports::wasi::sockets::network::ErrorCode::NewSocketLimit),
        11 => Some(bindings::exports::wasi::sockets::network::ErrorCode::AddressNotBindable),
        12 => Some(bindings::exports::wasi::sockets::network::ErrorCode::AddressInUse),
        13 => Some(bindings::exports::wasi::sockets::network::ErrorCode::RemoteUnreachable),
        14 => Some(bindings::exports::wasi::sockets::network::ErrorCode::ConnectionRefused),
        15 => Some(bindings::exports::wasi::sockets::network::ErrorCode::ConnectionReset),
        16 => Some(bindings::exports::wasi::sockets::network::ErrorCode::ConnectionAborted),
        17 => Some(bindings::exports::wasi::sockets::network::ErrorCode::DatagramTooLarge),
        18 => Some(bindings::exports::wasi::sockets::network::ErrorCode::NameUnresolvable),
        19 => Some(bindings::exports::wasi::sockets::network::ErrorCode::TemporaryResolverFailure),
        20 => Some(bindings::exports::wasi::sockets::network::ErrorCode::PermanentResolverFailure),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use proptest::proptest;
    use test_r::test;

    proptest! {

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
