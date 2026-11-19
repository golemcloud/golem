// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

use desert_rust::{
    BinaryDeserializer, BinaryInput, BinaryOutput, BinarySerializer, DeserializationContext,
    SerializationContext,
};
use wasmtime_wasi::p2::SocketError;
use wasmtime_wasi::p2::bindings::sockets::ip_name_lookup::IpAddress;
use wasmtime_wasi::p2::bindings::sockets::network::{
    ErrorCode as SocketErrorCode, IpSocketAddress,
};
use wasmtime_wasi::p3::bindings::sockets::{
    ip_name_lookup as p3_ip_name_lookup, types as p3_socket_types,
};

use super::{
    SerializableIpAddress, SerializableIpAddresses, SerializableP3IpNameLookupError,
    SerializableP3IpSocketAddress, SerializableP3SocketErrorCode, SerializableSocketError,
    SerializableSocketErrorCode, SerializableTcpStreamError, SerializableTcpStreamFailure,
};
use crate::schema::conversion::{FromSchemaError, SchemaBuilder, value_kind};
use crate::schema::metadata::TypeId;
use crate::schema::schema_type::SchemaType;
use crate::schema::schema_value::SchemaValue;
use wasmtime_wasi::StreamError;

impl SerializableSocketError {
    pub fn from_result(result: Result<SocketErrorCode, String>) -> Self {
        match result {
            Ok(error_code) => Self::ErrorCode(SerializableSocketErrorCode(error_code)),
            Err(msg) => SerializableSocketError::Generic(msg),
        }
    }
}

impl From<SocketError> for SerializableSocketError {
    fn from(value: SocketError) -> Self {
        Self::from_result(value.downcast().map_err(|err| err.to_string()))
    }
}

impl From<SerializableSocketError> for SocketError {
    fn from(value: SerializableSocketError) -> Self {
        match value {
            SerializableSocketError::ErrorCode(SerializableSocketErrorCode(error_code)) => {
                error_code.into()
            }
            SerializableSocketError::Generic(error) => {
                SocketError::trap(wasmtime::Error::msg(error))
            }
        }
    }
}

impl SerializableTcpStreamError {
    /// The recorded form of a P2 TCP stream error. A trap is never recorded, so it is returned.
    /// The native P2 TCP streams fail with a socket error code instead of an I/O error; the code
    /// is recorded, so the guest's `network-error-code` gives it live and on replay.
    pub fn from_stream_error(error: StreamError) -> Result<Self, wasmtime::Error> {
        match error {
            StreamError::Closed => Ok(Self::Closed),
            StreamError::LastOperationFailed(error) => {
                let io = error.downcast_ref::<std::io::Error>();
                let os_error = io.and_then(std::io::Error::raw_os_error);
                let code = match io {
                    Some(_) if os_error.is_some() => None,
                    Some(io) => Some(SocketErrorCode::from(io)),
                    None => error.downcast_ref::<SocketErrorCode>().copied(),
                };
                Ok(Self::LastOperationFailed(SerializableTcpStreamFailure {
                    os_error,
                    code: code.map(SerializableSocketErrorCode),
                    message: error.to_string(),
                }))
            }
            StreamError::Trap(error) => Err(error),
        }
    }
}

impl From<SerializableTcpStreamError> for StreamError {
    fn from(value: SerializableTcpStreamError) -> Self {
        match value {
            SerializableTcpStreamError::Closed => StreamError::Closed,
            SerializableTcpStreamError::LastOperationFailed(failure) => {
                let error = match (failure.os_error, failure.code) {
                    (Some(errno), _) => {
                        wasmtime::Error::from(std::io::Error::from_raw_os_error(errno))
                    }
                    (None, Some(SerializableSocketErrorCode(code))) => wasmtime::Error::from(
                        std::io::Error::new(io_error_kind(code), failure.message),
                    ),
                    (None, None) => wasmtime::Error::msg(failure.message),
                };
                StreamError::LastOperationFailed(error)
            }
        }
    }
}

/// The I/O error kind that gives `code` back as the guest's `network-error-code`.
fn io_error_kind(code: SocketErrorCode) -> std::io::ErrorKind {
    use std::io::ErrorKind;
    match code {
        SocketErrorCode::AddressInUse => ErrorKind::AddrInUse,
        SocketErrorCode::AddressNotBindable => ErrorKind::AddrNotAvailable,
        SocketErrorCode::ConnectionAborted => ErrorKind::ConnectionAborted,
        SocketErrorCode::ConnectionRefused => ErrorKind::ConnectionRefused,
        SocketErrorCode::ConnectionReset => ErrorKind::ConnectionReset,
        SocketErrorCode::InvalidArgument => ErrorKind::InvalidInput,
        SocketErrorCode::InvalidState => ErrorKind::NotConnected,
        SocketErrorCode::OutOfMemory => ErrorKind::OutOfMemory,
        SocketErrorCode::AccessDenied => ErrorKind::PermissionDenied,
        SocketErrorCode::Timeout => ErrorKind::TimedOut,
        SocketErrorCode::NotSupported => ErrorKind::Unsupported,
        SocketErrorCode::WouldBlock => ErrorKind::WouldBlock,
        _ => ErrorKind::Other,
    }
}

impl BinarySerializer for SerializableSocketErrorCode {
    fn serialize<Output: BinaryOutput>(
        &self,
        context: &mut SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        match &self.0 {
            SocketErrorCode::Unknown => context.write_u8(0),
            SocketErrorCode::AccessDenied => context.write_u8(1),
            SocketErrorCode::NotSupported => context.write_u8(2),
            SocketErrorCode::InvalidArgument => context.write_u8(3),
            SocketErrorCode::OutOfMemory => context.write_u8(4),
            SocketErrorCode::Timeout => context.write_u8(5),
            SocketErrorCode::ConcurrencyConflict => context.write_u8(6),
            SocketErrorCode::NotInProgress => context.write_u8(7),
            SocketErrorCode::WouldBlock => context.write_u8(8),
            SocketErrorCode::InvalidState => context.write_u8(9),
            SocketErrorCode::NewSocketLimit => context.write_u8(10),
            SocketErrorCode::AddressNotBindable => context.write_u8(11),
            SocketErrorCode::AddressInUse => context.write_u8(12),
            SocketErrorCode::RemoteUnreachable => context.write_u8(13),
            SocketErrorCode::ConnectionRefused => context.write_u8(14),
            SocketErrorCode::ConnectionReset => context.write_u8(15),
            SocketErrorCode::ConnectionAborted => context.write_u8(16),
            SocketErrorCode::DatagramTooLarge => context.write_u8(17),
            SocketErrorCode::NameUnresolvable => context.write_u8(18),
            SocketErrorCode::TemporaryResolverFailure => context.write_u8(19),
            SocketErrorCode::PermanentResolverFailure => context.write_u8(20),
        }
        Ok(())
    }
}

impl BinaryDeserializer for SerializableSocketErrorCode {
    fn deserialize(context: &mut DeserializationContext<'_>) -> desert_rust::Result<Self> {
        let tag = context.read_u8()?;
        let error_code = match tag {
            0 => SocketErrorCode::Unknown,
            1 => SocketErrorCode::AccessDenied,
            2 => SocketErrorCode::NotSupported,
            3 => SocketErrorCode::InvalidArgument,
            4 => SocketErrorCode::OutOfMemory,
            5 => SocketErrorCode::Timeout,
            6 => SocketErrorCode::ConcurrencyConflict,
            7 => SocketErrorCode::NotInProgress,
            8 => SocketErrorCode::WouldBlock,
            9 => SocketErrorCode::InvalidState,
            10 => SocketErrorCode::NewSocketLimit,
            11 => SocketErrorCode::AddressNotBindable,
            12 => SocketErrorCode::AddressInUse,
            13 => SocketErrorCode::RemoteUnreachable,
            14 => SocketErrorCode::ConnectionRefused,
            15 => SocketErrorCode::ConnectionReset,
            16 => SocketErrorCode::ConnectionAborted,
            17 => SocketErrorCode::DatagramTooLarge,
            18 => SocketErrorCode::NameUnresolvable,
            19 => SocketErrorCode::TemporaryResolverFailure,
            20 => SocketErrorCode::PermanentResolverFailure,
            other => {
                return Err(desert_rust::Error::DeserializationFailure(format!(
                    "Invalid tag for SerializableSocketErrorCode: {other}"
                )));
            }
        };
        Ok(SerializableSocketErrorCode(error_code))
    }
}

// Schema-native representation mirroring the legacy schema
// above: a flat enum with the same 21 cases/indices.
impl crate::schema::conversion::IntoSchema for SerializableSocketErrorCode {
    fn type_id() -> TypeId {
        TypeId::new("golem_common.model.oplog.payload.SerializableSocketErrorCode")
    }
    fn register_in(_b: &mut SchemaBuilder) -> SchemaType {
        SchemaType::r#enum(vec![
            "unknown".to_string(),
            "access-denied".to_string(),
            "not-supported".to_string(),
            "invalid-argument".to_string(),
            "out-of-memory".to_string(),
            "timeout".to_string(),
            "concurrency-conflict".to_string(),
            "not-in-progress".to_string(),
            "would-block".to_string(),
            "invalid-state".to_string(),
            "new-socket-limit".to_string(),
            "address-not-bindable".to_string(),
            "address-in-use".to_string(),
            "remote-unreachable".to_string(),
            "connection-refused".to_string(),
            "connection-reset".to_string(),
            "connection-aborted".to_string(),
            "datagram-too-large".to_string(),
            "name-unresolvable".to_string(),
            "temporary-resolver-failure".to_string(),
            "permanent-resolver-failure".to_string(),
        ])
    }
    fn to_value(&self) -> SchemaValue {
        let case = match &self.0 {
            SocketErrorCode::Unknown => 0,
            SocketErrorCode::AccessDenied => 1,
            SocketErrorCode::NotSupported => 2,
            SocketErrorCode::InvalidArgument => 3,
            SocketErrorCode::OutOfMemory => 4,
            SocketErrorCode::Timeout => 5,
            SocketErrorCode::ConcurrencyConflict => 6,
            SocketErrorCode::NotInProgress => 7,
            SocketErrorCode::WouldBlock => 8,
            SocketErrorCode::InvalidState => 9,
            SocketErrorCode::NewSocketLimit => 10,
            SocketErrorCode::AddressNotBindable => 11,
            SocketErrorCode::AddressInUse => 12,
            SocketErrorCode::RemoteUnreachable => 13,
            SocketErrorCode::ConnectionRefused => 14,
            SocketErrorCode::ConnectionReset => 15,
            SocketErrorCode::ConnectionAborted => 16,
            SocketErrorCode::DatagramTooLarge => 17,
            SocketErrorCode::NameUnresolvable => 18,
            SocketErrorCode::TemporaryResolverFailure => 19,
            SocketErrorCode::PermanentResolverFailure => 20,
        };
        SchemaValue::Enum { case }
    }
}

impl crate::schema::conversion::FromSchema for SerializableSocketErrorCode {
    fn from_value(v: &SchemaValue) -> Result<Self, FromSchemaError> {
        let case = match v {
            SchemaValue::Enum { case } => *case,
            other => {
                return Err(FromSchemaError::shape_mismatch(
                    "enum",
                    value_kind(other),
                    "SerializableSocketErrorCode",
                ));
            }
        };
        let error_code = match case {
            0 => SocketErrorCode::Unknown,
            1 => SocketErrorCode::AccessDenied,
            2 => SocketErrorCode::NotSupported,
            3 => SocketErrorCode::InvalidArgument,
            4 => SocketErrorCode::OutOfMemory,
            5 => SocketErrorCode::Timeout,
            6 => SocketErrorCode::ConcurrencyConflict,
            7 => SocketErrorCode::NotInProgress,
            8 => SocketErrorCode::WouldBlock,
            9 => SocketErrorCode::InvalidState,
            10 => SocketErrorCode::NewSocketLimit,
            11 => SocketErrorCode::AddressNotBindable,
            12 => SocketErrorCode::AddressInUse,
            13 => SocketErrorCode::RemoteUnreachable,
            14 => SocketErrorCode::ConnectionRefused,
            15 => SocketErrorCode::ConnectionReset,
            16 => SocketErrorCode::ConnectionAborted,
            17 => SocketErrorCode::DatagramTooLarge,
            18 => SocketErrorCode::NameUnresolvable,
            19 => SocketErrorCode::TemporaryResolverFailure,
            20 => SocketErrorCode::PermanentResolverFailure,
            other => {
                return Err(FromSchemaError::out_of_range(
                    other,
                    21,
                    "SerializableSocketErrorCode",
                ));
            }
        };
        Ok(SerializableSocketErrorCode(error_code))
    }
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

// Schema-native representation mirroring the legacy schema
// above: the address rendered as a string.
impl crate::schema::conversion::IntoSchema for SerializableIpAddress {
    fn type_id() -> TypeId {
        TypeId::new("golem_common.model.oplog.payload.SerializableIpAddress")
    }
    fn register_in(_b: &mut SchemaBuilder) -> SchemaType {
        SchemaType::string()
    }
    fn to_value(&self) -> SchemaValue {
        let addr = match self {
            SerializableIpAddress::IPv4 { address } => IpAddr::V4((*address).into()),
            SerializableIpAddress::IPv6 { address } => IpAddr::V6((*address).into()),
        };
        SchemaValue::String(addr.to_string())
    }
}

impl crate::schema::conversion::FromSchema for SerializableIpAddress {
    fn from_value(v: &SchemaValue) -> Result<Self, FromSchemaError> {
        match v {
            SchemaValue::String(s) => {
                let ipaddr =
                    IpAddr::from_str(s).map_err(|err| FromSchemaError::custom(err.to_string()))?;
                match ipaddr {
                    IpAddr::V4(addr) => Ok(SerializableIpAddress::IPv4 {
                        address: addr.octets(),
                    }),
                    IpAddr::V6(addr) => Ok(SerializableIpAddress::IPv6 {
                        address: addr.segments(),
                    }),
                }
            }
            other => Err(FromSchemaError::shape_mismatch(
                "string",
                value_kind(other),
                "SerializableIpAddress",
            )),
        }
    }
}

impl From<IpAddr> for SerializableIpAddress {
    fn from(value: IpAddr) -> Self {
        match value {
            IpAddr::V4(addr) => SerializableIpAddress::IPv4 {
                address: addr.octets(),
            },
            IpAddr::V6(addr) => SerializableIpAddress::IPv6 {
                address: addr.segments(),
            },
        }
    }
}

impl From<SerializableIpAddress> for IpAddr {
    fn from(value: SerializableIpAddress) -> Self {
        match value {
            SerializableIpAddress::IPv4 { address } => IpAddr::V4(Ipv4Addr::from(address)),
            SerializableIpAddress::IPv6 { address } => IpAddr::V6(Ipv6Addr::from(address)),
        }
    }
}

impl From<p3_socket_types::IpAddress> for SerializableIpAddress {
    fn from(value: p3_socket_types::IpAddress) -> Self {
        match value {
            p3_socket_types::IpAddress::Ipv4(address) => SerializableIpAddress::IPv4 {
                address: [address.0, address.1, address.2, address.3],
            },
            p3_socket_types::IpAddress::Ipv6(address) => SerializableIpAddress::IPv6 {
                address: [
                    address.0, address.1, address.2, address.3, address.4, address.5, address.6,
                    address.7,
                ],
            },
        }
    }
}

impl From<SerializableIpAddress> for p3_socket_types::IpAddress {
    fn from(value: SerializableIpAddress) -> Self {
        match value {
            SerializableIpAddress::IPv4 { address } => {
                p3_socket_types::IpAddress::Ipv4((address[0], address[1], address[2], address[3]))
            }
            SerializableIpAddress::IPv6 { address } => p3_socket_types::IpAddress::Ipv6((
                address[0], address[1], address[2], address[3], address[4], address[5], address[6],
                address[7],
            )),
        }
    }
}

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

impl From<Vec<p3_socket_types::IpAddress>> for SerializableIpAddresses {
    fn from(value: Vec<p3_socket_types::IpAddress>) -> Self {
        SerializableIpAddresses(value.into_iter().map(|v| v.into()).collect())
    }
}

impl From<SerializableIpAddresses> for Vec<p3_socket_types::IpAddress> {
    fn from(value: SerializableIpAddresses) -> Self {
        value.0.into_iter().map(|v| v.into()).collect()
    }
}

impl From<p3_socket_types::IpSocketAddress> for SerializableP3IpSocketAddress {
    fn from(value: p3_socket_types::IpSocketAddress) -> Self {
        match value {
            p3_socket_types::IpSocketAddress::Ipv4(address) => Self {
                address: p3_socket_types::IpAddress::Ipv4(address.address).into(),
                port: address.port,
                flow_info: None,
                scope_id: None,
            },
            p3_socket_types::IpSocketAddress::Ipv6(address) => Self {
                address: p3_socket_types::IpAddress::Ipv6(address.address).into(),
                port: address.port,
                flow_info: Some(address.flow_info),
                scope_id: Some(address.scope_id),
            },
        }
    }
}

impl From<IpSocketAddress> for SerializableP3IpSocketAddress {
    fn from(value: IpSocketAddress) -> Self {
        p3_socket_types::IpSocketAddress::from(std::net::SocketAddr::from(value)).into()
    }
}

impl From<SerializableP3IpSocketAddress> for IpSocketAddress {
    fn from(value: SerializableP3IpSocketAddress) -> Self {
        std::net::SocketAddr::from(p3_socket_types::IpSocketAddress::from(value)).into()
    }
}

impl From<SerializableP3IpSocketAddress> for p3_socket_types::IpSocketAddress {
    fn from(value: SerializableP3IpSocketAddress) -> Self {
        match p3_socket_types::IpAddress::from(value.address) {
            p3_socket_types::IpAddress::Ipv4(address) => {
                p3_socket_types::IpSocketAddress::Ipv4(p3_socket_types::Ipv4SocketAddress {
                    port: value.port,
                    address,
                })
            }
            p3_socket_types::IpAddress::Ipv6(address) => {
                p3_socket_types::IpSocketAddress::Ipv6(p3_socket_types::Ipv6SocketAddress {
                    port: value.port,
                    flow_info: value.flow_info.unwrap_or_default(),
                    address,
                    scope_id: value.scope_id.unwrap_or_default(),
                })
            }
        }
    }
}

impl From<p3_ip_name_lookup::ErrorCode> for SerializableP3IpNameLookupError {
    fn from(value: p3_ip_name_lookup::ErrorCode) -> Self {
        match value {
            p3_ip_name_lookup::ErrorCode::AccessDenied => Self::AccessDenied,
            p3_ip_name_lookup::ErrorCode::InvalidArgument => Self::InvalidArgument,
            p3_ip_name_lookup::ErrorCode::NameUnresolvable => Self::NameUnresolvable,
            p3_ip_name_lookup::ErrorCode::TemporaryResolverFailure => {
                Self::TemporaryResolverFailure
            }
            p3_ip_name_lookup::ErrorCode::PermanentResolverFailure => {
                Self::PermanentResolverFailure
            }
            p3_ip_name_lookup::ErrorCode::Other(error) => Self::Other(error),
        }
    }
}

impl From<SerializableP3IpNameLookupError> for p3_ip_name_lookup::ErrorCode {
    fn from(value: SerializableP3IpNameLookupError) -> Self {
        match value {
            SerializableP3IpNameLookupError::AccessDenied => Self::AccessDenied,
            SerializableP3IpNameLookupError::InvalidArgument => Self::InvalidArgument,
            SerializableP3IpNameLookupError::NameUnresolvable => Self::NameUnresolvable,
            SerializableP3IpNameLookupError::TemporaryResolverFailure => {
                Self::TemporaryResolverFailure
            }
            SerializableP3IpNameLookupError::PermanentResolverFailure => {
                Self::PermanentResolverFailure
            }
            SerializableP3IpNameLookupError::Other(error) => Self::Other(error),
        }
    }
}

impl From<p3_socket_types::ErrorCode> for SerializableP3SocketErrorCode {
    fn from(value: p3_socket_types::ErrorCode) -> Self {
        match value {
            p3_socket_types::ErrorCode::AccessDenied => Self::AccessDenied,
            p3_socket_types::ErrorCode::NotSupported => Self::NotSupported,
            p3_socket_types::ErrorCode::InvalidArgument => Self::InvalidArgument,
            p3_socket_types::ErrorCode::OutOfMemory => Self::OutOfMemory,
            p3_socket_types::ErrorCode::Timeout => Self::Timeout,
            p3_socket_types::ErrorCode::InvalidState => Self::InvalidState,
            p3_socket_types::ErrorCode::AddressNotBindable => Self::AddressNotBindable,
            p3_socket_types::ErrorCode::AddressInUse => Self::AddressInUse,
            p3_socket_types::ErrorCode::RemoteUnreachable => Self::RemoteUnreachable,
            p3_socket_types::ErrorCode::ConnectionRefused => Self::ConnectionRefused,
            p3_socket_types::ErrorCode::ConnectionBroken => Self::ConnectionBroken,
            p3_socket_types::ErrorCode::ConnectionReset => Self::ConnectionReset,
            p3_socket_types::ErrorCode::ConnectionAborted => Self::ConnectionAborted,
            p3_socket_types::ErrorCode::DatagramTooLarge => Self::DatagramTooLarge,
            p3_socket_types::ErrorCode::Other(error) => Self::Other(error),
        }
    }
}

impl From<SerializableP3SocketErrorCode> for p3_socket_types::ErrorCode {
    fn from(value: SerializableP3SocketErrorCode) -> Self {
        match value {
            SerializableP3SocketErrorCode::AccessDenied => Self::AccessDenied,
            SerializableP3SocketErrorCode::NotSupported => Self::NotSupported,
            SerializableP3SocketErrorCode::InvalidArgument => Self::InvalidArgument,
            SerializableP3SocketErrorCode::OutOfMemory => Self::OutOfMemory,
            SerializableP3SocketErrorCode::Timeout => Self::Timeout,
            SerializableP3SocketErrorCode::InvalidState => Self::InvalidState,
            SerializableP3SocketErrorCode::AddressNotBindable => Self::AddressNotBindable,
            SerializableP3SocketErrorCode::AddressInUse => Self::AddressInUse,
            SerializableP3SocketErrorCode::RemoteUnreachable => Self::RemoteUnreachable,
            SerializableP3SocketErrorCode::ConnectionRefused => Self::ConnectionRefused,
            SerializableP3SocketErrorCode::ConnectionBroken => Self::ConnectionBroken,
            SerializableP3SocketErrorCode::ConnectionReset => Self::ConnectionReset,
            SerializableP3SocketErrorCode::ConnectionAborted => Self::ConnectionAborted,
            SerializableP3SocketErrorCode::DatagramTooLarge => Self::DatagramTooLarge,
            SerializableP3SocketErrorCode::Other(error) => Self::Other(error),
        }
    }
}
