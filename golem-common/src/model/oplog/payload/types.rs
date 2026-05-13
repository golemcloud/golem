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

use crate::base_model::TransactionId;
use crate::model::agent::UntypedDataValue;
use crate::model::component::ComponentRevision;
use crate::model::environment::EnvironmentId;
use crate::model::invocation_context::AttributeValue;
use crate::model::oplog::{
    PublicAttribute, PublicExternalSpanData, PublicLocalSpanData, PublicSpanData, SpanData,
};
use crate::model::worker::TypedAgentConfigEntry;
use crate::model::{AgentId, AgentMetadata, AgentStatus, RdbmsPoolKey, ScheduleId};
use bigdecimal::BigDecimal;
use bit_vec::BitVec;
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use desert_rust::{
    BinaryCodec, BinaryDeserializer, BinaryInput, BinaryOutput, BinarySerializer,
    DeserializationContext, SerializationContext,
};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::analysis::analysed_type::{r#enum, str, tuple};
use golem_wasm::{FromValue, IntoValue, NodeIndex, Value};
use golem_wasm_derive::{FromValue, IntoValue};
use http::Version;
use mac_address::MacAddress;
use serde::{Deserialize, Serialize};
use sqlx::ValueRef;
use sqlx::postgres::PgTypeKind;
use sqlx::postgres::types::{Oid, PgInterval, PgRange, PgTimeTz};
use std::collections::{BTreeMap, HashMap};
use std::fmt::{Debug, Display, Formatter};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::ops::Add;
use std::ops::Bound;
use std::str::FromStr;
use std::time::{Duration, SystemTime};
use uuid::Uuid;
use wasmtime_wasi::StreamError;

// TODO(p3) Blocker 3: removed all wasmtime p2 imports (filesystem, sockets, FsError, SocketError,
// http types, HostIncomingBody, HostIncomingResponse, FieldMap) and the conversions that depended
// on them. They will be re-added against p3 wasmtime types when HTTP / filesystem / sockets
// durability is re-implemented on top of WASI p3.

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub struct ObjectMetadata {
    pub name: String,
    pub container: String,
    pub created_at: u64,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub struct SerializableDateTime {
    pub seconds: u64,
    pub nanoseconds: u32,
}

impl From<golem_wasm::wasi::clocks::system_clock::Instant> for SerializableDateTime {
    fn from(value: golem_wasm::wasi::clocks::system_clock::Instant) -> Self {
        Self {
            seconds: value.seconds.max(0) as u64,
            nanoseconds: value.nanoseconds,
        }
    }
}

impl From<SerializableDateTime> for wasmtime_wasi::p3::bindings::clocks::system_clock::Instant {
    fn from(value: SerializableDateTime) -> Self {
        Self {
            seconds: value.seconds as i64,
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

impl From<SerializableDateTime> for DateTime<Utc> {
    fn from(value: SerializableDateTime) -> Self {
        Self::from(SystemTime::from(value))
    }
}

impl From<DateTime<Utc>> for SerializableDateTime {
    fn from(value: DateTime<Utc>) -> Self {
        SystemTime::from(value).into()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub struct SerializableFileTimes {
    pub data_access_timestamp: Option<SerializableDateTime>,
    pub data_modification_timestamp: Option<SerializableDateTime>,
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub enum FileSystemError {
    ErrorCode(SerializableFsErrorCode),
    Generic(String),
}

impl FileSystemError {
    pub fn from_result(result: Result<SerializableFsErrorCode, String>) -> Self {
        match result {
            Ok(error_code) => Self::ErrorCode(error_code),
            Err(msg) => FileSystemError::Generic(msg),
        }
    }
}

// TODO(p3) Blocker 3: removed `impl From<FileSystemError> for wasmtime_wasi::p2::FsError`
// pending p3-native filesystem durability support.

/// Mirror of the WASI filesystem `error-code` variant. Kept as a local enum so the oplog
/// payload is not coupled to a specific wasmtime bindings version.
///
/// The numeric tags below MUST match the previous `SerializableFsErrorCode` binary layout
/// to preserve oplog backward compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerializableFsErrorCode {
    Access,
    WouldBlock,
    Already,
    BadDescriptor,
    Busy,
    Deadlock,
    Quota,
    Exist,
    FileTooLarge,
    IllegalByteSequence,
    InProgress,
    Interrupted,
    Invalid,
    Io,
    IsDirectory,
    Loop,
    TooManyLinks,
    MessageSize,
    NameTooLong,
    NoDevice,
    NoEntry,
    NoLock,
    InsufficientMemory,
    InsufficientSpace,
    NotDirectory,
    NotEmpty,
    NotRecoverable,
    Unsupported,
    NoTty,
    NoSuchDevice,
    Overflow,
    NotPermitted,
    Pipe,
    ReadOnly,
    InvalidSeek,
    TextFileBusy,
    CrossDevice,
}

impl BinarySerializer for SerializableFsErrorCode {
    fn serialize<Output: BinaryOutput>(
        &self,
        context: &mut SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        match self {
            SerializableFsErrorCode::Access => context.write_u8(0),
            SerializableFsErrorCode::WouldBlock => context.write_u8(1),
            SerializableFsErrorCode::Already => context.write_u8(2),
            SerializableFsErrorCode::BadDescriptor => context.write_u8(3),
            SerializableFsErrorCode::Busy => context.write_u8(4),
            SerializableFsErrorCode::Deadlock => context.write_u8(5),
            SerializableFsErrorCode::Quota => context.write_u8(6),
            SerializableFsErrorCode::Exist => context.write_u8(7),
            SerializableFsErrorCode::FileTooLarge => context.write_u8(8),
            SerializableFsErrorCode::IllegalByteSequence => context.write_u8(9),
            SerializableFsErrorCode::InProgress => context.write_u8(10),
            SerializableFsErrorCode::Interrupted => context.write_u8(11),
            SerializableFsErrorCode::Invalid => context.write_u8(12),
            SerializableFsErrorCode::Io => context.write_u8(13),
            SerializableFsErrorCode::IsDirectory => context.write_u8(14),
            SerializableFsErrorCode::Loop => context.write_u8(15),
            SerializableFsErrorCode::TooManyLinks => context.write_u8(16),
            SerializableFsErrorCode::MessageSize => context.write_u8(17),
            SerializableFsErrorCode::NameTooLong => context.write_u8(18),
            SerializableFsErrorCode::NoDevice => context.write_u8(19),
            SerializableFsErrorCode::NoEntry => context.write_u8(20),
            SerializableFsErrorCode::NoLock => context.write_u8(21),
            SerializableFsErrorCode::InsufficientMemory => context.write_u8(22),
            SerializableFsErrorCode::InsufficientSpace => context.write_u8(23),
            SerializableFsErrorCode::NotDirectory => context.write_u8(24),
            SerializableFsErrorCode::NotEmpty => context.write_u8(25),
            SerializableFsErrorCode::NotRecoverable => context.write_u8(26),
            SerializableFsErrorCode::Unsupported => context.write_u8(27),
            SerializableFsErrorCode::NoTty => context.write_u8(28),
            SerializableFsErrorCode::NoSuchDevice => context.write_u8(29),
            SerializableFsErrorCode::Overflow => context.write_u8(30),
            SerializableFsErrorCode::NotPermitted => context.write_u8(31),
            SerializableFsErrorCode::Pipe => context.write_u8(32),
            SerializableFsErrorCode::ReadOnly => context.write_u8(33),
            SerializableFsErrorCode::InvalidSeek => context.write_u8(34),
            SerializableFsErrorCode::TextFileBusy => context.write_u8(35),
            SerializableFsErrorCode::CrossDevice => context.write_u8(36),
        }
        Ok(())
    }
}

impl BinaryDeserializer for SerializableFsErrorCode {
    fn deserialize(context: &mut DeserializationContext<'_>) -> desert_rust::Result<Self> {
        let tag = context.read_u8()?;
        let error_code = match tag {
            0 => SerializableFsErrorCode::Access,
            1 => SerializableFsErrorCode::WouldBlock,
            2 => SerializableFsErrorCode::Already,
            3 => SerializableFsErrorCode::BadDescriptor,
            4 => SerializableFsErrorCode::Busy,
            5 => SerializableFsErrorCode::Deadlock,
            6 => SerializableFsErrorCode::Quota,
            7 => SerializableFsErrorCode::Exist,
            8 => SerializableFsErrorCode::FileTooLarge,
            9 => SerializableFsErrorCode::IllegalByteSequence,
            10 => SerializableFsErrorCode::InProgress,
            11 => SerializableFsErrorCode::Interrupted,
            12 => SerializableFsErrorCode::Invalid,
            13 => SerializableFsErrorCode::Io,
            14 => SerializableFsErrorCode::IsDirectory,
            15 => SerializableFsErrorCode::Loop,
            16 => SerializableFsErrorCode::TooManyLinks,
            17 => SerializableFsErrorCode::MessageSize,
            18 => SerializableFsErrorCode::NameTooLong,
            19 => SerializableFsErrorCode::NoDevice,
            20 => SerializableFsErrorCode::NoEntry,
            21 => SerializableFsErrorCode::NoLock,
            22 => SerializableFsErrorCode::InsufficientMemory,
            23 => SerializableFsErrorCode::InsufficientSpace,
            24 => SerializableFsErrorCode::NotDirectory,
            25 => SerializableFsErrorCode::NotEmpty,
            26 => SerializableFsErrorCode::NotRecoverable,
            27 => SerializableFsErrorCode::Unsupported,
            28 => SerializableFsErrorCode::NoTty,
            29 => SerializableFsErrorCode::NoSuchDevice,
            30 => SerializableFsErrorCode::Overflow,
            31 => SerializableFsErrorCode::NotPermitted,
            32 => SerializableFsErrorCode::Pipe,
            33 => SerializableFsErrorCode::ReadOnly,
            34 => SerializableFsErrorCode::InvalidSeek,
            35 => SerializableFsErrorCode::TextFileBusy,
            36 => SerializableFsErrorCode::CrossDevice,
            other => {
                return Err(desert_rust::Error::DeserializationFailure(format!(
                    "Invalid tag for SerializableFsErrorCode: {other}"
                )));
            }
        };
        Ok(error_code)
    }
}

impl IntoValue for SerializableFsErrorCode {
    fn into_value(self) -> Value {
        match self {
            SerializableFsErrorCode::Access => Value::Enum(0),
            SerializableFsErrorCode::WouldBlock => Value::Enum(1),
            SerializableFsErrorCode::Already => Value::Enum(2),
            SerializableFsErrorCode::BadDescriptor => Value::Enum(3),
            SerializableFsErrorCode::Busy => Value::Enum(4),
            SerializableFsErrorCode::Deadlock => Value::Enum(5),
            SerializableFsErrorCode::Quota => Value::Enum(6),
            SerializableFsErrorCode::Exist => Value::Enum(7),
            SerializableFsErrorCode::FileTooLarge => Value::Enum(8),
            SerializableFsErrorCode::IllegalByteSequence => Value::Enum(9),
            SerializableFsErrorCode::InProgress => Value::Enum(10),
            SerializableFsErrorCode::Interrupted => Value::Enum(11),
            SerializableFsErrorCode::Invalid => Value::Enum(12),
            SerializableFsErrorCode::Io => Value::Enum(13),
            SerializableFsErrorCode::IsDirectory => Value::Enum(14),
            SerializableFsErrorCode::Loop => Value::Enum(15),
            SerializableFsErrorCode::TooManyLinks => Value::Enum(16),
            SerializableFsErrorCode::MessageSize => Value::Enum(17),
            SerializableFsErrorCode::NameTooLong => Value::Enum(18),
            SerializableFsErrorCode::NoDevice => Value::Enum(19),
            SerializableFsErrorCode::NoEntry => Value::Enum(20),
            SerializableFsErrorCode::NoLock => Value::Enum(21),
            SerializableFsErrorCode::InsufficientMemory => Value::Enum(22),
            SerializableFsErrorCode::InsufficientSpace => Value::Enum(23),
            SerializableFsErrorCode::NotDirectory => Value::Enum(24),
            SerializableFsErrorCode::NotEmpty => Value::Enum(25),
            SerializableFsErrorCode::NotRecoverable => Value::Enum(26),
            SerializableFsErrorCode::Unsupported => Value::Enum(27),
            SerializableFsErrorCode::NoTty => Value::Enum(28),
            SerializableFsErrorCode::NoSuchDevice => Value::Enum(29),
            SerializableFsErrorCode::Overflow => Value::Enum(30),
            SerializableFsErrorCode::NotPermitted => Value::Enum(31),
            SerializableFsErrorCode::Pipe => Value::Enum(32),
            SerializableFsErrorCode::ReadOnly => Value::Enum(33),
            SerializableFsErrorCode::InvalidSeek => Value::Enum(34),
            SerializableFsErrorCode::TextFileBusy => Value::Enum(35),
            SerializableFsErrorCode::CrossDevice => Value::Enum(36),
        }
    }

    fn get_type() -> AnalysedType {
        r#enum(&[
            "access",
            "would-block",
            "already",
            "bad-descriptor",
            "busy",
            "deadlock",
            "quota",
            "exist",
            "file-too-large",
            "illegal-byte-sequence",
            "in-progress",
            "interrupted",
            "invalid",
            "io",
            "is-directory",
            "loop",
            "too-many-links",
            "message-size",
            "name-too-long",
            "no-device",
            "no-entry",
            "no-lock",
            "insufficient-memory",
            "insufficient-space",
            "not-directory",
            "not-empty",
            "not-recoverable",
            "unsupported",
            "no-tty",
            "no-such-device",
            "overflow",
            "not-permitted",
            "pipe",
            "read-only",
            "invalid-seek",
            "text-file-busy",
            "cross-device",
        ])
    }
}

impl FromValue for SerializableFsErrorCode {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Enum(0) => Ok(SerializableFsErrorCode::Access),
            Value::Enum(1) => Ok(SerializableFsErrorCode::WouldBlock),
            Value::Enum(2) => Ok(SerializableFsErrorCode::Already),
            Value::Enum(3) => Ok(SerializableFsErrorCode::BadDescriptor),
            Value::Enum(4) => Ok(SerializableFsErrorCode::Busy),
            Value::Enum(5) => Ok(SerializableFsErrorCode::Deadlock),
            Value::Enum(6) => Ok(SerializableFsErrorCode::Quota),
            Value::Enum(7) => Ok(SerializableFsErrorCode::Exist),
            Value::Enum(8) => Ok(SerializableFsErrorCode::FileTooLarge),
            Value::Enum(9) => Ok(SerializableFsErrorCode::IllegalByteSequence),
            Value::Enum(10) => Ok(SerializableFsErrorCode::InProgress),
            Value::Enum(11) => Ok(SerializableFsErrorCode::Interrupted),
            Value::Enum(12) => Ok(SerializableFsErrorCode::Invalid),
            Value::Enum(13) => Ok(SerializableFsErrorCode::Io),
            Value::Enum(14) => Ok(SerializableFsErrorCode::IsDirectory),
            Value::Enum(15) => Ok(SerializableFsErrorCode::Loop),
            Value::Enum(16) => Ok(SerializableFsErrorCode::TooManyLinks),
            Value::Enum(17) => Ok(SerializableFsErrorCode::MessageSize),
            Value::Enum(18) => Ok(SerializableFsErrorCode::NameTooLong),
            Value::Enum(19) => Ok(SerializableFsErrorCode::NoDevice),
            Value::Enum(20) => Ok(SerializableFsErrorCode::NoEntry),
            Value::Enum(21) => Ok(SerializableFsErrorCode::NoLock),
            Value::Enum(22) => Ok(SerializableFsErrorCode::InsufficientMemory),
            Value::Enum(23) => Ok(SerializableFsErrorCode::InsufficientSpace),
            Value::Enum(24) => Ok(SerializableFsErrorCode::NotDirectory),
            Value::Enum(25) => Ok(SerializableFsErrorCode::NotEmpty),
            Value::Enum(26) => Ok(SerializableFsErrorCode::NotRecoverable),
            Value::Enum(27) => Ok(SerializableFsErrorCode::Unsupported),
            Value::Enum(28) => Ok(SerializableFsErrorCode::NoTty),
            Value::Enum(29) => Ok(SerializableFsErrorCode::NoSuchDevice),
            Value::Enum(30) => Ok(SerializableFsErrorCode::Overflow),
            Value::Enum(31) => Ok(SerializableFsErrorCode::NotPermitted),
            Value::Enum(32) => Ok(SerializableFsErrorCode::Pipe),
            Value::Enum(33) => Ok(SerializableFsErrorCode::ReadOnly),
            Value::Enum(34) => Ok(SerializableFsErrorCode::InvalidSeek),
            Value::Enum(35) => Ok(SerializableFsErrorCode::TextFileBusy),
            Value::Enum(36) => Ok(SerializableFsErrorCode::CrossDevice),
            _ => Err(format!(
                "Invalid value for SerializableFsErrorCode: {:?}",
                value
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub enum SerializableSocketError {
    ErrorCode(SerializableSocketErrorCode),
    Generic(String),
}

impl SerializableSocketError {
    pub fn from_result(result: Result<SerializableSocketErrorCode, String>) -> Self {
        match result {
            Ok(error_code) => Self::ErrorCode(error_code),
            Err(msg) => SerializableSocketError::Generic(msg),
        }
    }
}

// TODO(p3) Blocker 3: removed `impl From<wasmtime_wasi::p2::SocketError> for SerializableSocketError`
// and `impl From<SerializableSocketError> for wasmtime_wasi::p2::SocketError` pending p3-native
// sockets durability support.

/// Mirror of the WASI sockets `error-code` variant. Kept as a local enum so the oplog payload is
/// not coupled to a specific wasmtime bindings version.
///
/// The numeric tags below MUST match the previous `SerializableSocketErrorCode` binary layout
/// to preserve oplog backward compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerializableSocketErrorCode {
    Unknown,
    AccessDenied,
    NotSupported,
    InvalidArgument,
    OutOfMemory,
    Timeout,
    ConcurrencyConflict,
    NotInProgress,
    WouldBlock,
    InvalidState,
    NewSocketLimit,
    AddressNotBindable,
    AddressInUse,
    RemoteUnreachable,
    ConnectionRefused,
    ConnectionReset,
    ConnectionAborted,
    DatagramTooLarge,
    NameUnresolvable,
    TemporaryResolverFailure,
    PermanentResolverFailure,
}

impl BinarySerializer for SerializableSocketErrorCode {
    fn serialize<Output: BinaryOutput>(
        &self,
        context: &mut SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        match self {
            SerializableSocketErrorCode::Unknown => context.write_u8(0),
            SerializableSocketErrorCode::AccessDenied => context.write_u8(1),
            SerializableSocketErrorCode::NotSupported => context.write_u8(2),
            SerializableSocketErrorCode::InvalidArgument => context.write_u8(3),
            SerializableSocketErrorCode::OutOfMemory => context.write_u8(4),
            SerializableSocketErrorCode::Timeout => context.write_u8(5),
            SerializableSocketErrorCode::ConcurrencyConflict => context.write_u8(6),
            SerializableSocketErrorCode::NotInProgress => context.write_u8(7),
            SerializableSocketErrorCode::WouldBlock => context.write_u8(8),
            SerializableSocketErrorCode::InvalidState => context.write_u8(9),
            SerializableSocketErrorCode::NewSocketLimit => context.write_u8(10),
            SerializableSocketErrorCode::AddressNotBindable => context.write_u8(11),
            SerializableSocketErrorCode::AddressInUse => context.write_u8(12),
            SerializableSocketErrorCode::RemoteUnreachable => context.write_u8(13),
            SerializableSocketErrorCode::ConnectionRefused => context.write_u8(14),
            SerializableSocketErrorCode::ConnectionReset => context.write_u8(15),
            SerializableSocketErrorCode::ConnectionAborted => context.write_u8(16),
            SerializableSocketErrorCode::DatagramTooLarge => context.write_u8(17),
            SerializableSocketErrorCode::NameUnresolvable => context.write_u8(18),
            SerializableSocketErrorCode::TemporaryResolverFailure => context.write_u8(19),
            SerializableSocketErrorCode::PermanentResolverFailure => context.write_u8(20),
        }
        Ok(())
    }
}

impl BinaryDeserializer for SerializableSocketErrorCode {
    fn deserialize(context: &mut DeserializationContext<'_>) -> desert_rust::Result<Self> {
        let tag = context.read_u8()?;
        let error_code = match tag {
            0 => SerializableSocketErrorCode::Unknown,
            1 => SerializableSocketErrorCode::AccessDenied,
            2 => SerializableSocketErrorCode::NotSupported,
            3 => SerializableSocketErrorCode::InvalidArgument,
            4 => SerializableSocketErrorCode::OutOfMemory,
            5 => SerializableSocketErrorCode::Timeout,
            6 => SerializableSocketErrorCode::ConcurrencyConflict,
            7 => SerializableSocketErrorCode::NotInProgress,
            8 => SerializableSocketErrorCode::WouldBlock,
            9 => SerializableSocketErrorCode::InvalidState,
            10 => SerializableSocketErrorCode::NewSocketLimit,
            11 => SerializableSocketErrorCode::AddressNotBindable,
            12 => SerializableSocketErrorCode::AddressInUse,
            13 => SerializableSocketErrorCode::RemoteUnreachable,
            14 => SerializableSocketErrorCode::ConnectionRefused,
            15 => SerializableSocketErrorCode::ConnectionReset,
            16 => SerializableSocketErrorCode::ConnectionAborted,
            17 => SerializableSocketErrorCode::DatagramTooLarge,
            18 => SerializableSocketErrorCode::NameUnresolvable,
            19 => SerializableSocketErrorCode::TemporaryResolverFailure,
            20 => SerializableSocketErrorCode::PermanentResolverFailure,
            other => {
                return Err(desert_rust::Error::DeserializationFailure(format!(
                    "Invalid tag for SerializableSocketErrorCode: {other}"
                )));
            }
        };
        Ok(error_code)
    }
}

impl IntoValue for SerializableSocketErrorCode {
    fn into_value(self) -> Value {
        match self {
            SerializableSocketErrorCode::Unknown => Value::Enum(0),
            SerializableSocketErrorCode::AccessDenied => Value::Enum(1),
            SerializableSocketErrorCode::NotSupported => Value::Enum(2),
            SerializableSocketErrorCode::InvalidArgument => Value::Enum(3),
            SerializableSocketErrorCode::OutOfMemory => Value::Enum(4),
            SerializableSocketErrorCode::Timeout => Value::Enum(5),
            SerializableSocketErrorCode::ConcurrencyConflict => Value::Enum(6),
            SerializableSocketErrorCode::NotInProgress => Value::Enum(7),
            SerializableSocketErrorCode::WouldBlock => Value::Enum(8),
            SerializableSocketErrorCode::InvalidState => Value::Enum(9),
            SerializableSocketErrorCode::NewSocketLimit => Value::Enum(10),
            SerializableSocketErrorCode::AddressNotBindable => Value::Enum(11),
            SerializableSocketErrorCode::AddressInUse => Value::Enum(12),
            SerializableSocketErrorCode::RemoteUnreachable => Value::Enum(13),
            SerializableSocketErrorCode::ConnectionRefused => Value::Enum(14),
            SerializableSocketErrorCode::ConnectionReset => Value::Enum(15),
            SerializableSocketErrorCode::ConnectionAborted => Value::Enum(16),
            SerializableSocketErrorCode::DatagramTooLarge => Value::Enum(17),
            SerializableSocketErrorCode::NameUnresolvable => Value::Enum(18),
            SerializableSocketErrorCode::TemporaryResolverFailure => Value::Enum(19),
            SerializableSocketErrorCode::PermanentResolverFailure => Value::Enum(20),
        }
    }

    fn get_type() -> AnalysedType {
        r#enum(&[
            "unknown",
            "access-denied",
            "not-supported",
            "invalid-argument",
            "out-of-memory",
            "timeout",
            "concurrency-conflict",
            "not-in-progress",
            "would-block",
            "invalid-state",
            "new-socket-limit",
            "address-not-bindable",
            "address-in-use",
            "remote-unreachable",
            "connection-refused",
            "connection-reset",
            "connection-aborted",
            "datagram-too-large",
            "name-unresolvable",
            "temporary-resolver-failure",
            "permanent-resolver-failure",
        ])
    }
}

impl FromValue for SerializableSocketErrorCode {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::Enum(0) => Ok(SerializableSocketErrorCode::Unknown),
            Value::Enum(1) => Ok(SerializableSocketErrorCode::AccessDenied),
            Value::Enum(2) => Ok(SerializableSocketErrorCode::NotSupported),
            Value::Enum(3) => Ok(SerializableSocketErrorCode::InvalidArgument),
            Value::Enum(4) => Ok(SerializableSocketErrorCode::OutOfMemory),
            Value::Enum(5) => Ok(SerializableSocketErrorCode::Timeout),
            Value::Enum(6) => Ok(SerializableSocketErrorCode::ConcurrencyConflict),
            Value::Enum(7) => Ok(SerializableSocketErrorCode::NotInProgress),
            Value::Enum(8) => Ok(SerializableSocketErrorCode::WouldBlock),
            Value::Enum(9) => Ok(SerializableSocketErrorCode::InvalidState),
            Value::Enum(10) => Ok(SerializableSocketErrorCode::NewSocketLimit),
            Value::Enum(11) => Ok(SerializableSocketErrorCode::AddressNotBindable),
            Value::Enum(12) => Ok(SerializableSocketErrorCode::AddressInUse),
            Value::Enum(13) => Ok(SerializableSocketErrorCode::RemoteUnreachable),
            Value::Enum(14) => Ok(SerializableSocketErrorCode::ConnectionRefused),
            Value::Enum(15) => Ok(SerializableSocketErrorCode::ConnectionReset),
            Value::Enum(16) => Ok(SerializableSocketErrorCode::ConnectionAborted),
            Value::Enum(17) => Ok(SerializableSocketErrorCode::DatagramTooLarge),
            Value::Enum(18) => Ok(SerializableSocketErrorCode::NameUnresolvable),
            Value::Enum(19) => Ok(SerializableSocketErrorCode::TemporaryResolverFailure),
            Value::Enum(20) => Ok(SerializableSocketErrorCode::PermanentResolverFailure),
            _ => Err(format!(
                "Invalid value for SerializableSocketErrorCode: {:?}",
                value
            )),
        }
    }
}

#[derive(Debug, Clone, BinaryCodec)]
pub enum SerializableHttpVersion {
    Http09,
    /// `HTTP/1.0`
    Http10,
    /// `HTTP/1.1`
    Http11,
    /// `HTTP/2.0`
    Http2,
    /// `HTTP/3.0`
    Http3,
}

impl TryFrom<Version> for SerializableHttpVersion {
    type Error = String;

    fn try_from(value: Version) -> Result<Self, Self::Error> {
        if value == Version::HTTP_09 {
            Ok(SerializableHttpVersion::Http09)
        } else if value == Version::HTTP_10 {
            Ok(SerializableHttpVersion::Http10)
        } else if value == Version::HTTP_11 {
            Ok(SerializableHttpVersion::Http11)
        } else if value == Version::HTTP_2 {
            Ok(SerializableHttpVersion::Http2)
        } else if value == Version::HTTP_3 {
            Ok(SerializableHttpVersion::Http3)
        } else {
            Err(format!("Unknown HTTP version: {value:?}"))
        }
    }
}

impl From<SerializableHttpVersion> for Version {
    fn from(value: SerializableHttpVersion) -> Self {
        match value {
            SerializableHttpVersion::Http09 => Version::HTTP_09,
            SerializableHttpVersion::Http10 => Version::HTTP_10,
            SerializableHttpVersion::Http11 => Version::HTTP_11,
            SerializableHttpVersion::Http2 => Version::HTTP_2,
            SerializableHttpVersion::Http3 => Version::HTTP_3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub enum SerializableHttpResponse {
    Pending,
    HeadersReceived(SerializableResponseHeaders),
    HttpError(SerializableHttpErrorCode),
    InternalError(Option<String>),
}

#[derive(Debug, Clone, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub struct SerializableResponseHeaders {
    pub status: u16,
    pub headers: HashMap<String, Vec<Vec<u8>>>,
}

// TODO(p3) Blocker 3: removed `impl TryFrom<&HostIncomingResponse> for SerializableResponseHeaders`
// and `impl TryFrom<SerializableResponseHeaders> for HostIncomingResponse` (used the
// wasmtime_wasi_http p2 `HostIncomingResponse`, `HostIncomingBody`, and `FieldMap` types) pending
// p3-native HTTP durability support.

#[derive(Debug, Clone, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub struct SerializableTlsAlertReceivedPayload {
    pub alert_id: Option<u8>,
    pub alert_message: Option<String>,
}

// TODO(p3) Blocker 3: removed `impl From<&TlsAlertReceivedPayload> for SerializableTlsAlertReceivedPayload`
// and `impl From<SerializableTlsAlertReceivedPayload> for TlsAlertReceivedPayload` (wasmtime
// p2 HTTP types) pending p3-native HTTP durability support.

#[derive(Debug, Clone, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub struct SerializableDnsErrorPayload {
    pub rcode: Option<String>,
    pub info_code: Option<u16>,
}

// TODO(p3) Blocker 3: removed `impl From<&DnsErrorPayload> for SerializableDnsErrorPayload`
// and `impl From<SerializableDnsErrorPayload> for DnsErrorPayload` (wasmtime p2 HTTP types)
// pending p3-native HTTP durability support.

#[derive(Debug, Clone, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub struct SerializableFieldSizePayload {
    pub field_name: Option<String>,
    pub field_size: Option<u32>,
}

// TODO(p3) Blocker 3: removed `impl From<&FieldSizePayload> for SerializableFieldSizePayload`
// and `impl From<SerializableFieldSizePayload> for FieldSizePayload` (wasmtime p2 HTTP types)
// pending p3-native HTTP durability support.

#[derive(Debug, Clone, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub enum SerializableHttpErrorCode {
    DnsTimeout,
    DnsError(SerializableDnsErrorPayload),
    DestinationNotFound,
    DestinationUnavailable,
    DestinationIpProhibited,
    DestinationIpUnroutable,
    ConnectionRefused,
    ConnectionTerminated,
    ConnectionTimeout,
    ConnectionReadTimeout,
    ConnectionWriteTimeout,
    ConnectionLimitReached,
    TlsProtocolError,
    TlsCertificateError,
    TlsAlertReceived(SerializableTlsAlertReceivedPayload),
    HttpRequestDenied,
    HttpRequestLengthRequired,
    HttpRequestBodySize(Option<u64>),
    HttpRequestMethodInvalid,
    HttpRequestUriInvalid,
    HttpRequestUriTooLong,
    HttpRequestHeaderSectionSize(Option<u32>),
    HttpRequestHeaderSize(Option<SerializableFieldSizePayload>),
    HttpRequestTrailerSectionSize(Option<u32>),
    HttpRequestTrailerSize(SerializableFieldSizePayload),
    HttpResponseIncomplete,
    HttpResponseHeaderSectionSize(Option<u32>),
    HttpResponseHeaderSize(SerializableFieldSizePayload),
    HttpResponseBodySize(Option<u64>),
    HttpResponseTrailerSectionSize(Option<u32>),
    HttpResponseTrailerSize(SerializableFieldSizePayload),
    HttpResponseTransferCoding(Option<String>),
    HttpResponseContentCoding(Option<String>),
    HttpResponseTimeout,
    HttpUpgradeFailed,
    HttpProtocolError,
    LoopDetected,
    ConfigurationError,
    InternalError(Option<String>),
}

// TODO(p3) Blocker 3: removed
//   - `impl From<wasmtime_wasi_http::p2::bindings::http::types::ErrorCode> for SerializableHttpErrorCode`
//   - `impl From<&wasmtime_wasi_http::p2::bindings::http::types::ErrorCode> for SerializableHttpErrorCode`
//   - `impl From<SerializableHttpErrorCode> for wasmtime_wasi_http::p2::bindings::http::types::ErrorCode`
// pending p3-native HTTP durability support.

#[derive(Debug, Clone, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub enum SerializableHttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Head,
    Connect,
    Options,
    Trace,
    Patch,
    Other(String),
}

// TODO(p3) Blocker 3: removed `impl From<wasmtime_wasi_http::p2::bindings::http::types::Method>
// for SerializableHttpMethod` pending p3-native HTTP durability support.

impl TryFrom<&SerializableHttpMethod> for http::Method {
    type Error = anyhow::Error;

    fn try_from(value: &SerializableHttpMethod) -> Result<Self, Self::Error> {
        match value {
            SerializableHttpMethod::Get => Ok(http::Method::GET),
            SerializableHttpMethod::Post => Ok(http::Method::POST),
            SerializableHttpMethod::Put => Ok(http::Method::PUT),
            SerializableHttpMethod::Delete => Ok(http::Method::DELETE),
            SerializableHttpMethod::Head => Ok(http::Method::HEAD),
            SerializableHttpMethod::Connect => Ok(http::Method::CONNECT),
            SerializableHttpMethod::Options => Ok(http::Method::OPTIONS),
            SerializableHttpMethod::Trace => Ok(http::Method::TRACE),
            SerializableHttpMethod::Patch => Ok(http::Method::PATCH),
            SerializableHttpMethod::Other(m) => http::Method::from_bytes(m.as_bytes())
                .map_err(|e| anyhow::anyhow!("invalid HTTP method '{m}': {e}")),
        }
    }
}

impl Display for SerializableHttpMethod {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SerializableHttpMethod::Get => write!(f, "GET"),
            SerializableHttpMethod::Post => write!(f, "POST"),
            SerializableHttpMethod::Put => write!(f, "PUT"),
            SerializableHttpMethod::Delete => write!(f, "DELETE"),
            SerializableHttpMethod::Head => write!(f, "HEAD"),
            SerializableHttpMethod::Connect => write!(f, "CONNECT"),
            SerializableHttpMethod::Options => write!(f, "OPTIONS"),
            SerializableHttpMethod::Trace => write!(f, "TRACE"),
            SerializableHttpMethod::Patch => write!(f, "PATCH"),
            SerializableHttpMethod::Other(method) => write!(f, "{method}"),
        }
    }
}

/// A subset of AgentMetadata visible for guests (and serializable to oplog)
#[derive(Debug, Clone, PartialEq, IntoValue, FromValue, BinaryCodec)]
pub struct AgentMetadataForGuests {
    pub agent_id: AgentId,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub config: BTreeMap<String, String>,
    pub status: AgentStatus,
    pub component_revision: ComponentRevision,
    pub retry_count: u64,
    pub environment_id: EnvironmentId,
}

impl From<AgentMetadata> for AgentMetadataForGuests {
    fn from(value: AgentMetadata) -> Self {
        Self {
            agent_id: value.agent_id,
            args: vec![],
            env: value.env,
            config: TypedAgentConfigEntry::to_flat_map(&value.config),
            status: value.last_known_status.status,
            component_revision: value.last_known_status.component_revision,
            retry_count: value
                .last_known_status
                .current_retry_state
                .iter()
                .max_by_key(|(idx, _)| **idx)
                .map(|(_, state)| state.retry_count())
                .unwrap_or_default() as u64,
            environment_id: value.environment_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub enum SerializableStreamError {
    Closed,
    LastOperationFailed(String),
    Trap(String),
}

impl From<StreamError> for SerializableStreamError {
    fn from(value: StreamError) -> Self {
        match value {
            StreamError::Closed => Self::Closed,
            StreamError::LastOperationFailed(e) => Self::LastOperationFailed(e.to_string()),
            StreamError::Trap(e) => Self::Trap(e.to_string()),
        }
    }
}

impl From<SerializableStreamError> for StreamError {
    fn from(value: SerializableStreamError) -> Self {
        match value {
            SerializableStreamError::Closed => Self::Closed,
            SerializableStreamError::LastOperationFailed(e) => {
                Self::LastOperationFailed(wasmtime::Error::msg(e))
            }
            SerializableStreamError::Trap(e) => Self::Trap(wasmtime::Error::msg(e)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub enum SerializableIpAddress {
    IPv4 { address: [u8; 4] },
    IPv6 { address: [u16; 8] },
}

// TODO(p3) Blocker 3: removed `impl From<wasmtime_wasi::p2::bindings::sockets::ip_name_lookup::IpAddress>
// for SerializableIpAddress` and `impl From<SerializableIpAddress> for ...IpAddress` pending
// p3-native sockets durability support.

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

impl FromValue for SerializableIpAddress {
    fn from_value(value: Value) -> Result<Self, String> {
        let str = String::from_value(value)?;
        let ipaddr = IpAddr::from_str(&str).map_err(|err| err.to_string())?;
        match ipaddr {
            IpAddr::V4(addr) => Ok(SerializableIpAddress::IPv4 {
                address: addr.octets(),
            }),
            IpAddr::V6(addr) => Ok(SerializableIpAddress::IPv6 {
                address: addr.segments(),
            }),
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

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec, IntoValue, FromValue)]
#[desert(transparent)]
pub struct SerializableIpAddresses(pub Vec<SerializableIpAddress>);

// TODO(p3) Blocker 3: removed `impl From<Vec<wasmtime_wasi::p2::bindings::sockets::ip_name_lookup::IpAddress>>
// for SerializableIpAddresses` and `impl From<SerializableIpAddresses> for Vec<...IpAddress>`
// pending p3-native sockets durability support.

#[derive(Debug, Clone, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub enum SerializableInvokeResult {
    Failed(String),
    Pending,
    Completed(Result<UntypedDataValue, SerializableRpcError>),
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub enum SerializableRpcError {
    ProtocolError { details: String },
    Denied { details: String },
    NotFound { details: String },
    RemoteInternalError { details: String },
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub struct SerializableWebsocketCloseInfo {
    pub code: u16,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub enum SerializableWebsocketMessage {
    Text(String),
    Binary(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub enum SerializableWebsocketError {
    ConnectionFailure(String),
    SendFailure(String),
    ReceiveFailure(String),
    ProtocolError(String),
    Closed(Option<SerializableWebsocketCloseInfo>),
    Other(String),
}

#[derive(Debug, Clone, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
#[wit_transparent]
pub struct SerializableScheduleId {
    pub id: Uuid,
}

impl SerializableScheduleId {
    pub fn from_domain(schedule_id: ScheduleId) -> Self {
        Self { id: schedule_id.id }
    }

    pub fn into_domain(self) -> ScheduleId {
        ScheduleId { id: self.id }
    }
}

pub fn encode_span_data(spans: &[SpanData]) -> Vec<Vec<PublicSpanData>> {
    let mut result = Vec::new();
    let mut current = Vec::new();

    for span in spans.iter().rev() {
        match span {
            SpanData::LocalSpan {
                span_id,
                start,
                parent_id,
                linked_context,
                attributes,
                inherited,
            } => {
                let linked_context = if let Some(linked_context) = linked_context {
                    let mut encoded_linked_context = encode_span_data(linked_context);

                    // Before merging encoded_linked_context into result, we need to adjust the indices in it
                    for spans in encoded_linked_context.iter_mut() {
                        for span in spans.iter_mut() {
                            match span {
                                PublicSpanData::LocalSpan(local_span) => {
                                    if let Some(idx) = local_span.linked_context.as_mut() {
                                        *idx += (result.len() as u64) + 1;
                                    }
                                }
                                PublicSpanData::ExternalSpan(_) => {}
                            }
                        }
                    }

                    result.extend(encoded_linked_context);

                    let id = result.len() as u64 + 1;
                    Some(id)
                } else {
                    None
                };
                let span_data = PublicSpanData::LocalSpan(PublicLocalSpanData {
                    span_id: span_id.clone(),
                    start: *start,
                    parent_id: parent_id.clone(),
                    linked_context,
                    attributes: attributes
                        .iter()
                        .map(|(k, v)| PublicAttribute {
                            key: k.clone(),
                            value: v.clone().into(),
                        })
                        .collect(),
                    inherited: *inherited,
                });
                current.insert(0, span_data);
            }
            SpanData::ExternalSpan { span_id } => {
                let span_data = PublicSpanData::ExternalSpan(PublicExternalSpanData {
                    span_id: span_id.clone(),
                });
                current.insert(0, span_data);
            }
        }
    }

    for stack in &mut result {
        for span in stack {
            if let PublicSpanData::LocalSpan(local_span) = span
                && let Some(linked_id) = &mut local_span.linked_context
            {
                *linked_id += 1;
            }
        }
    }
    result.insert(0, current);
    result
}

pub fn decode_span_data(spans: Vec<Vec<PublicSpanData>>) -> Vec<SpanData> {
    let mut result = Vec::new();
    let mut linked_contexts = Vec::new();

    for stack in spans {
        linked_contexts.push(stack);
    }

    if !linked_contexts.is_empty() {
        let current = linked_contexts.remove(0);
        for span in current {
            match span {
                PublicSpanData::LocalSpan(local_span) => {
                    let linked_context = if let Some(idx) = local_span.linked_context {
                        let linked_idx = (idx - 1) as usize;
                        if linked_idx < linked_contexts.len() {
                            Some(decode_span_data(vec![linked_contexts[linked_idx].clone()]))
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    result.push(SpanData::LocalSpan {
                        span_id: local_span.span_id,
                        start: local_span.start,
                        parent_id: local_span.parent_id,
                        linked_context,
                        attributes: local_span
                            .attributes
                            .into_iter()
                            .map(|attr| (attr.key, AttributeValue::from(attr.value)))
                            .collect(),
                        inherited: local_span.inherited,
                    });
                }
                PublicSpanData::ExternalSpan(external_span) => {
                    result.push(SpanData::ExternalSpan {
                        span_id: external_span.span_id,
                    });
                }
            }
        }
    }

    result
}

#[derive(Clone, Debug, Eq, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub enum SerializableRdbmsError {
    ConnectionFailure(String),
    QueryParameterFailure(String),
    QueryExecutionFailure(String),
    QueryResponseFailure(String),
    Other(String),
}

#[derive(Clone, Debug, PartialEq, BinaryCodec)]
#[desert(transparent)]
pub struct SerializableMacAddress(pub MacAddress);

impl IntoValue for SerializableMacAddress {
    fn into_value(self) -> Value {
        Value::String(self.0.to_string())
    }

    fn get_type() -> AnalysedType {
        str()
    }
}

impl FromValue for SerializableMacAddress {
    fn from_value(value: Value) -> Result<Self, String> {
        let str = String::from_value(value)?;
        let macaddr = MacAddress::from_str(&str).map_err(|err| err.to_string())?;
        Ok(SerializableMacAddress(macaddr))
    }
}

impl From<MacAddress> for SerializableMacAddress {
    fn from(value: MacAddress) -> Self {
        SerializableMacAddress(value)
    }
}

impl From<SerializableMacAddress> for MacAddress {
    fn from(value: SerializableMacAddress) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub struct SerializableRdbmsRequest {
    pub pool_key: RdbmsPoolKey,
    pub statement: String,
    pub params: Vec<SerializableDbValue>,
    pub transaction_id: Option<TransactionId>,
}

#[derive(Clone, Debug, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub struct SerializableDbValue {
    pub nodes: Vec<SerializableDbValueNode>,
}

#[derive(Clone, Debug, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub enum SerializableDbValueNode {
    Boolean(bool),
    Tinyint(i8),
    Smallint(i16),
    Mediumint(i32),
    Int(i32),
    Bigint(i64),
    TinyintUnsigned(u8),
    SmallintUnsigned(u16),
    MediumintUnsigned(u32),
    IntUnsigned(u32),
    BigintUnsigned(u64),
    Float(f32),
    Double(f64),
    Decimal(BigDecimal),
    Date(NaiveDate),
    Datetimetz(DateTime<Utc>),
    Timestamp(NaiveDateTime),
    Timestamptz(DateTime<Utc>),
    Time(chrono::NaiveTime),
    Timetz(TimeTz),
    Interval(Interval),
    Year(u16),
    Bpchar(String),
    Varchar(String),
    Tinytext(String),
    Text(String),
    Mediumtext(String),
    Longtext(String),
    Binary(Vec<u8>),
    Varbinary(Vec<u8>),
    Tinyblob(Vec<u8>),
    Blob(Vec<u8>),
    Mediumblob(Vec<u8>),
    Longblob(Vec<u8>),
    Bytea(Vec<u8>),
    Json(String),
    Jsonb(String),
    Jsonpath(String),
    Xml(String),
    Uuid(Uuid),
    Inet(SerializableIpAddress),
    Cidr(SerializableIpAddress),
    Macaddr(SerializableMacAddress),
    Bit(BitVec),
    Varbit(BitVec),
    Int4range(ValuesRange<i32>),
    Int8range(ValuesRange<i64>),
    Numrange(ValuesRange<BigDecimal>),
    Tsrange(ValuesRange<NaiveDateTime>),
    Tstzrange(ValuesRange<DateTime<Utc>>),
    Daterange(ValuesRange<NaiveDate>),
    Money(i64),
    Oid(u32),
    Enumeration(Enumeration),
    Composite(SerializableComposite),
    Domain(SerializableDomain),
    Array(Vec<NodeIndex>),
    Range(SerializableRange),
    Set(String),
    Null,
    Vector(Vec<f32>),
    Halfvec(Vec<f32>),
    Sparsevec(SparseVec),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, BinaryCodec)]
#[desert(evolution())]
pub struct ValuesRange<T> {
    pub start: Bound<T>,
    pub end: Bound<T>,
}

impl<T> ValuesRange<T> {
    pub fn new(start: Bound<T>, end: Bound<T>) -> Self {
        ValuesRange { start, end }
    }

    pub fn start_value(&self) -> Option<&T> {
        match &self.start {
            Bound::Included(v) => Some(v),
            Bound::Excluded(v) => Some(v),
            Bound::Unbounded => None,
        }
    }

    pub fn end_value(&self) -> Option<&T> {
        match &self.end {
            Bound::Included(v) => Some(v),
            Bound::Excluded(v) => Some(v),
            Bound::Unbounded => None,
        }
    }

    pub fn map<U>(self, f: impl Fn(T) -> U + Clone) -> ValuesRange<U> {
        let start: Bound<U> = self.start.map(f.clone());
        let end: Bound<U> = self.end.map(f.clone());
        ValuesRange::new(start, end)
    }

    pub fn try_map<U>(
        self,
        f: impl Fn(T) -> Result<U, String> + Clone,
    ) -> Result<ValuesRange<U>, String> {
        fn to_bound<T, U>(
            v: Bound<T>,
            f: impl Fn(T) -> Result<U, String>,
        ) -> Result<Bound<U>, String> {
            match v {
                Bound::Included(v) => Ok(Bound::Included(f(v)?)),
                Bound::Excluded(v) => Ok(Bound::Excluded(f(v)?)),
                Bound::Unbounded => Ok(Bound::Unbounded),
            }
        }
        let start: Bound<U> = to_bound(self.start, f.clone())?;
        let end: Bound<U> = to_bound(self.end, f.clone())?;

        Ok(ValuesRange::new(start, end))
    }
}

impl<T: Debug> Display for ValuesRange<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?} {:?}", self.start, self.end)
    }
}

impl<T: IntoValue> IntoValue for ValuesRange<T> {
    fn into_value(self) -> Value {
        Value::Tuple(vec![self.start.into_value(), self.end.into_value()])
    }

    fn get_type() -> AnalysedType {
        tuple(vec![T::get_type(), T::get_type()])
    }
}

impl<T: FromValue> FromValue for ValuesRange<T> {
    fn from_value(value: Value) -> Result<Self, String> {
        let mut tuple = match value {
            Value::Tuple(elements) => elements,
            _ => return Err("Expected Tuple value".to_string()),
        };

        if tuple.len() != 2 {
            return Err("Expected Tuple of length 2".to_string());
        }

        let start = Bound::from_value(tuple.remove(0))?;
        let end = Bound::from_value(tuple.remove(0))?;

        Ok(ValuesRange::new(start, end))
    }
}

#[derive(Clone, Debug, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub struct Interval {
    pub months: i32,
    pub days: i32,
    pub microseconds: i64,
}

impl Interval {
    pub fn new(months: i32, days: i32, microseconds: i64) -> Self {
        Interval {
            months,
            days,
            microseconds,
        }
    }
}

impl Display for Interval {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}m {}d {}us", self.months, self.days, self.microseconds)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub struct TimeTz {
    pub time: chrono::NaiveTime,
    pub offset: i32,
}

impl TimeTz {
    pub fn new(time: chrono::NaiveTime, offset: chrono::FixedOffset) -> Self {
        TimeTz {
            time,
            offset: offset.utc_minus_local(),
        }
    }
}

impl Display for TimeTz {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}{}", self.time, self.offset)
    }
}

#[derive(Clone, Debug, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub struct Enumeration {
    pub name: String,
    pub value: String,
}

impl Enumeration {
    pub fn new(name: String, value: String) -> Self {
        Enumeration { name, value }
    }
}

impl Display for Enumeration {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}({})", self.name, self.value)
    }
}

#[derive(Clone, Debug, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub struct SerializableComposite {
    pub name: String,
    pub values: Vec<NodeIndex>,
}

#[derive(Clone, Debug, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub struct SerializableDomain {
    pub name: String,
    pub value: NodeIndex,
}

#[derive(Clone, Debug, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub struct SerializableRange {
    pub name: String,
    pub value: ValuesRange<NodeIndex>,
}

#[derive(Clone, Debug, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub struct SparseVec {
    pub dim: i32,
    pub indices: Vec<i32>,
    pub values: Vec<f32>,
}

impl SparseVec {
    pub fn try_new(dim: i32, indices: Vec<i32>, values: Vec<f32>) -> Result<Self, String> {
        if indices.len() != values.len() {
            Err("Indices and values must have the same length".to_string())
        } else if indices.len() > dim as usize {
            Err("Indices must be less than or equal to dim".to_string())
        } else {
            Ok(SparseVec {
                dim,
                indices,
                values,
            })
        }
    }

    pub fn to_map(&self) -> HashMap<&i32, &f32> {
        let mut map = HashMap::new();
        for (idx, val) in self.indices.iter().zip(self.values.iter()) {
            map.insert(idx, val);
        }
        map
    }
}

impl Display for SparseVec {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{{")?;
        for (i, (idx, val)) in self.indices.iter().zip(self.values.iter()).enumerate() {
            if i > 0 {
                write!(f, ",")?;
            }
            write!(f, "{}:{}", idx, val)?;
        }
        write!(f, "}}/{}", self.dim)
    }
}

impl<T> From<ValuesRange<T>> for PgRange<T> {
    fn from(range: ValuesRange<T>) -> Self {
        PgRange {
            start: range.start,
            end: range.end,
        }
    }
}

impl<T> From<PgRange<T>> for ValuesRange<T> {
    fn from(range: PgRange<T>) -> Self {
        ValuesRange {
            start: range.start,
            end: range.end,
        }
    }
}

impl From<PgInterval> for Interval {
    fn from(interval: PgInterval) -> Self {
        Self {
            months: interval.months,
            days: interval.days,
            microseconds: interval.microseconds,
        }
    }
}

impl From<Interval> for PgInterval {
    fn from(interval: Interval) -> Self {
        Self {
            months: interval.months,
            days: interval.days,
            microseconds: interval.microseconds,
        }
    }
}

impl From<PgTimeTz> for TimeTz {
    fn from(value: PgTimeTz) -> Self {
        Self {
            time: value.time,
            offset: value.offset.utc_minus_local(),
        }
    }
}

impl TryFrom<TimeTz> for PgTimeTz {
    type Error = String;
    fn try_from(value: TimeTz) -> Result<Self, Self::Error> {
        let offset = chrono::offset::FixedOffset::west_opt(value.offset)
            .ok_or("Offset value is not valid")?;
        Ok(Self {
            time: value.time,
            offset,
        })
    }
}

impl sqlx::types::Type<sqlx::Postgres> for Enumeration {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <&str as sqlx::types::Type<sqlx::Postgres>>::type_info()
    }

    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        matches!(ty.kind(), PgTypeKind::Enum(_))
    }
}

impl<'r> sqlx::Decode<'r, sqlx::Postgres> for Enumeration {
    fn decode(
        value: sqlx::postgres::PgValueRef<'r>,
    ) -> Result<Self, Box<dyn std::error::Error + 'static + Send + Sync>> {
        use sqlx::TypeInfo;

        let type_info = &value.type_info();
        let name = type_info.name().to_string();
        if matches!(type_info.kind(), PgTypeKind::Enum(_)) {
            let v = <String as sqlx::Decode<sqlx::Postgres>>::decode(value)?;
            Ok(Enumeration::new(name, v))
        } else {
            Err(format!("Type '{name}' is not supported").into())
        }
    }
}

impl sqlx::Encode<'_, sqlx::Postgres> for Enumeration {
    fn encode_by_ref(
        &self,
        buf: &mut sqlx::postgres::PgArgumentBuffer,
    ) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <String as sqlx::Encode<sqlx::Postgres>>::encode_by_ref(&self.value, buf)
    }

    fn produces(&self) -> Option<sqlx::postgres::PgTypeInfo> {
        Some(sqlx::postgres::PgTypeInfo::with_name(
            self.name.clone().leak(),
        ))
    }
}

impl sqlx::postgres::PgHasArrayType for Enumeration {
    fn array_type_info() -> sqlx::postgres::PgTypeInfo {
        sqlx::postgres::PgTypeInfo::with_oid(Oid(2277)) // pseudo type array
    }

    fn array_compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        matches!(ty.kind(), PgTypeKind::Array(ty) if <Enumeration as sqlx::types::Type<sqlx::Postgres>>::compatible(ty))
    }
}

#[derive(Clone, Debug, Eq, PartialEq, BinaryCodec, IntoValue, FromValue)]
pub struct EnumerationType {
    pub name: String,
}

impl EnumerationType {
    pub fn new(name: String) -> Self {
        EnumerationType { name }
    }
}

impl Display for EnumerationType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

#[derive(Clone, Debug, PartialEq, BinaryCodec, IntoValue, FromValue)]
pub struct SerializableCompositeType {
    pub name: String,
    pub attributes: Vec<(String, NodeIndex)>,
}

#[derive(Clone, Debug, PartialEq, BinaryCodec, IntoValue, FromValue)]
pub struct SerializableDomainType {
    pub name: String,
    pub base_type: NodeIndex,
}

#[derive(Clone, Debug, PartialEq, BinaryCodec, IntoValue, FromValue)]
pub struct SerializableRangeType {
    pub name: String,
    pub base_type: NodeIndex,
}

#[derive(Clone, Debug, PartialEq, BinaryCodec, IntoValue, FromValue)]
pub struct SerializableDbColumnType {
    pub nodes: Vec<SerializableDbColumnTypeNode>,
}

#[derive(Clone, Debug, PartialEq, BinaryCodec, IntoValue, FromValue)]
pub enum SerializableDbColumnTypeNode {
    Boolean,
    Tinyint,
    Smallint,
    Mediumint,
    Int,
    Bigint,
    TinyintUnsigned,
    SmallintUnsigned,
    MediumintUnsigned,
    IntUnsigned,
    BigintUnsigned,
    Float,
    Double,
    Decimal,
    Date,
    Datetime,
    Timestamp,
    Time,
    Year,
    Fixchar,
    Varchar,
    Tinytext,
    Text,
    Mediumtext,
    Longtext,
    Binary,
    Varbinary,
    Tinyblob,
    Blob,
    Mediumblob,
    Longblob,
    Set,
    Bit,
    Json,
    Character,
    Int2,
    Int4,
    Int8,
    Float4,
    Float8,
    Numeric,
    Bpchar,
    Timestamptz,
    Timetz,
    Interval,
    Bytea,
    Uuid,
    Xml,
    Jsonb,
    Jsonpath,
    Inet,
    Cidr,
    Macaddr,
    Varbit,
    Int4range,
    Int8range,
    Numrange,
    Tsrange,
    Tstzrange,
    Daterange,
    Money,
    Oid,
    Enumeration(EnumerationType),
    Composite(SerializableCompositeType),
    Domain(SerializableDomainType),
    Array(NodeIndex),
    Range(SerializableRangeType),
    Null,
    Vector,
    Halfvec,
    Sparsevec,
}

#[derive(Clone, Debug, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub struct SerializableDbColumn {
    pub ordinal: u64,
    pub name: String,
    pub db_type: SerializableDbColumnType,
    pub db_type_name: String,
}

#[derive(Clone, Debug, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub struct SerializableDbResult {
    pub columns: Vec<SerializableDbColumn>,
    pub rows: Vec<Vec<SerializableDbValue>>,
}

#[cfg(test)]
mod serializable_http_method_tests {
    use super::*;
    use test_r::test;

    #[test]
    fn test_serializable_method_to_http() {
        assert_eq!(
            http::Method::try_from(&SerializableHttpMethod::Get).unwrap(),
            http::Method::GET
        );
        assert_eq!(
            http::Method::try_from(&SerializableHttpMethod::Post).unwrap(),
            http::Method::POST
        );
        assert_eq!(
            http::Method::try_from(&SerializableHttpMethod::Other("PURGE".to_string())).unwrap(),
            http::Method::from_bytes(b"PURGE").unwrap()
        );
    }
}
