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

use crate::model::invocation_context::{AttributeValue, InvocationContextStack, TraceId};
use crate::model::oplog::public_oplog_entry::BinaryCodec;
use crate::model::oplog::{
    PublicAttribute, PublicExternalSpanData, PublicLocalSpanData, PublicSpanData,
    SpanData,
};
use crate::model::{
    AccountId, ComponentVersion, IdempotencyKey, OwnedWorkerId, ProjectId, ScheduleId,
    ScheduledAction, WorkerId, WorkerMetadata, WorkerStatus,
};
use anyhow::anyhow;
use desert_rust::{
    BinaryDeserializer, BinaryInput, BinaryOutput, BinarySerializer, DeserializationContext,
    SerializationContext,
};
use golem_wasm::analysis::analysed_type::{r#enum, str};
use golem_wasm::analysis::AnalysedType;
use golem_wasm::{FromValue, IntoValue, Value, ValueAndType};
use golem_wasm_derive::{FromValue, IntoValue};
use http::{HeaderName, HeaderValue, Version};
use std::collections::{BTreeMap, HashMap};
use std::fmt::{Display, Formatter};
use std::net::IpAddr;
use std::ops::Add;
use std::str::FromStr;
use std::time::{Duration, SystemTime};
use wasmtime_wasi::p2::bindings::filesystem;
use wasmtime_wasi::p2::bindings::sockets::ip_name_lookup::IpAddress;
use wasmtime_wasi::p2::bindings::sockets::network::ErrorCode as SocketErrorCode;
use wasmtime_wasi::p2::{FsError, SocketError};
use wasmtime_wasi::StreamError;
use wasmtime_wasi_http::bindings::http::types::{
    DnsErrorPayload, FieldSizePayload, Method, TlsAlertReceivedPayload,
};
use wasmtime_wasi_http::body::HostIncomingBody;
use wasmtime_wasi_http::types::{FieldMap, HostIncomingResponse};

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

impl From<wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime> for SerializableDateTime {
    fn from(value: wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime) -> Self {
        Self {
            seconds: value.seconds,
            nanoseconds: value.nanoseconds,
        }
    }
}

impl From<SerializableDateTime> for wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime {
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
    pub fn from_result(result: Result<filesystem::types::ErrorCode, String>) -> Self {
        match result {
            Ok(error_code) => Self::ErrorCode(SerializableFsErrorCode(error_code)),
            Err(msg) => FileSystemError::Generic(msg),
        }
    }
}

impl From<FileSystemError> for FsError {
    fn from(value: FileSystemError) -> Self {
        match value {
            FileSystemError::ErrorCode(SerializableFsErrorCode(error_code)) => error_code.into(),
            FileSystemError::Generic(error) => FsError::trap(anyhow!(error)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerializableFsErrorCode(filesystem::types::ErrorCode);

impl BinarySerializer for SerializableFsErrorCode {
    fn serialize<Output: BinaryOutput>(
        &self,
        context: &mut SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        match &self.0 {
            filesystem::types::ErrorCode::Access => context.write_u8(0),
            filesystem::types::ErrorCode::WouldBlock => context.write_u8(1),
            filesystem::types::ErrorCode::Already => context.write_u8(2),
            filesystem::types::ErrorCode::BadDescriptor => context.write_u8(3),
            filesystem::types::ErrorCode::Busy => context.write_u8(4),
            filesystem::types::ErrorCode::Deadlock => context.write_u8(5),
            filesystem::types::ErrorCode::Quota => context.write_u8(6),
            filesystem::types::ErrorCode::Exist => context.write_u8(7),
            filesystem::types::ErrorCode::FileTooLarge => context.write_u8(8),
            filesystem::types::ErrorCode::IllegalByteSequence => context.write_u8(9),
            filesystem::types::ErrorCode::InProgress => context.write_u8(10),
            filesystem::types::ErrorCode::Interrupted => context.write_u8(11),
            filesystem::types::ErrorCode::Invalid => context.write_u8(12),
            filesystem::types::ErrorCode::Io => context.write_u8(13),
            filesystem::types::ErrorCode::IsDirectory => context.write_u8(14),
            filesystem::types::ErrorCode::Loop => context.write_u8(15),
            filesystem::types::ErrorCode::TooManyLinks => context.write_u8(16),
            filesystem::types::ErrorCode::MessageSize => context.write_u8(17),
            filesystem::types::ErrorCode::NameTooLong => context.write_u8(18),
            filesystem::types::ErrorCode::NoDevice => context.write_u8(19),
            filesystem::types::ErrorCode::NoEntry => context.write_u8(20),
            filesystem::types::ErrorCode::NoLock => context.write_u8(21),
            filesystem::types::ErrorCode::InsufficientMemory => context.write_u8(22),
            filesystem::types::ErrorCode::InsufficientSpace => context.write_u8(23),
            filesystem::types::ErrorCode::NotDirectory => context.write_u8(24),
            filesystem::types::ErrorCode::NotEmpty => context.write_u8(25),
            filesystem::types::ErrorCode::NotRecoverable => context.write_u8(26),
            filesystem::types::ErrorCode::Unsupported => context.write_u8(27),
            filesystem::types::ErrorCode::NoTty => context.write_u8(28),
            filesystem::types::ErrorCode::NoSuchDevice => context.write_u8(29),
            filesystem::types::ErrorCode::Overflow => context.write_u8(30),
            filesystem::types::ErrorCode::NotPermitted => context.write_u8(31),
            filesystem::types::ErrorCode::Pipe => context.write_u8(32),
            filesystem::types::ErrorCode::ReadOnly => context.write_u8(33),
            filesystem::types::ErrorCode::InvalidSeek => context.write_u8(34),
            filesystem::types::ErrorCode::TextFileBusy => context.write_u8(35),
            filesystem::types::ErrorCode::CrossDevice => context.write_u8(36),
        }
        Ok(())
    }
}

impl BinaryDeserializer for SerializableFsErrorCode {
    fn deserialize(context: &mut DeserializationContext<'_>) -> desert_rust::Result<Self> {
        let tag = context.read_u8()?;
        let error_code = match tag {
            0 => filesystem::types::ErrorCode::Access,
            1 => filesystem::types::ErrorCode::WouldBlock,
            2 => filesystem::types::ErrorCode::Already,
            3 => filesystem::types::ErrorCode::BadDescriptor,
            4 => filesystem::types::ErrorCode::Busy,
            5 => filesystem::types::ErrorCode::Deadlock,
            6 => filesystem::types::ErrorCode::Quota,
            7 => filesystem::types::ErrorCode::Exist,
            8 => filesystem::types::ErrorCode::FileTooLarge,
            9 => filesystem::types::ErrorCode::IllegalByteSequence,
            10 => filesystem::types::ErrorCode::InProgress,
            11 => filesystem::types::ErrorCode::Interrupted,
            12 => filesystem::types::ErrorCode::Invalid,
            13 => filesystem::types::ErrorCode::Io,
            14 => filesystem::types::ErrorCode::IsDirectory,
            15 => filesystem::types::ErrorCode::Loop,
            16 => filesystem::types::ErrorCode::TooManyLinks,
            17 => filesystem::types::ErrorCode::MessageSize,
            18 => filesystem::types::ErrorCode::NameTooLong,
            19 => filesystem::types::ErrorCode::NoDevice,
            20 => filesystem::types::ErrorCode::NoEntry,
            21 => filesystem::types::ErrorCode::NoLock,
            22 => filesystem::types::ErrorCode::InsufficientMemory,
            23 => filesystem::types::ErrorCode::InsufficientSpace,
            24 => filesystem::types::ErrorCode::NotDirectory,
            25 => filesystem::types::ErrorCode::NotEmpty,
            26 => filesystem::types::ErrorCode::NotRecoverable,
            27 => filesystem::types::ErrorCode::Unsupported,
            28 => filesystem::types::ErrorCode::NoTty,
            29 => filesystem::types::ErrorCode::NoSuchDevice,
            30 => filesystem::types::ErrorCode::Overflow,
            31 => filesystem::types::ErrorCode::NotPermitted,
            32 => filesystem::types::ErrorCode::Pipe,
            33 => filesystem::types::ErrorCode::ReadOnly,
            34 => filesystem::types::ErrorCode::InvalidSeek,
            35 => filesystem::types::ErrorCode::TextFileBusy,
            36 => filesystem::types::ErrorCode::CrossDevice,
            other => {
                return Err(desert_rust::Error::DeserializationFailure(format!(
                    "Invalid tag for SerializableFsErrorCode: {other}"
                )))
            }
        };
        Ok(SerializableFsErrorCode(error_code))
    }
}

impl IntoValue for SerializableFsErrorCode {
    fn into_value(self) -> Value {
        match &self.0 {
            filesystem::types::ErrorCode::Access => Value::Enum(0),
            filesystem::types::ErrorCode::WouldBlock => Value::Enum(1),
            filesystem::types::ErrorCode::Already => Value::Enum(2),
            filesystem::types::ErrorCode::BadDescriptor => Value::Enum(3),
            filesystem::types::ErrorCode::Busy => Value::Enum(4),
            filesystem::types::ErrorCode::Deadlock => Value::Enum(5),
            filesystem::types::ErrorCode::Quota => Value::Enum(6),
            filesystem::types::ErrorCode::Exist => Value::Enum(7),
            filesystem::types::ErrorCode::FileTooLarge => Value::Enum(8),
            filesystem::types::ErrorCode::IllegalByteSequence => Value::Enum(9),
            filesystem::types::ErrorCode::InProgress => Value::Enum(10),
            filesystem::types::ErrorCode::Interrupted => Value::Enum(11),
            filesystem::types::ErrorCode::Invalid => Value::Enum(12),
            filesystem::types::ErrorCode::Io => Value::Enum(13),
            filesystem::types::ErrorCode::IsDirectory => Value::Enum(14),
            filesystem::types::ErrorCode::Loop => Value::Enum(15),
            filesystem::types::ErrorCode::TooManyLinks => Value::Enum(16),
            filesystem::types::ErrorCode::MessageSize => Value::Enum(17),
            filesystem::types::ErrorCode::NameTooLong => Value::Enum(18),
            filesystem::types::ErrorCode::NoDevice => Value::Enum(19),
            filesystem::types::ErrorCode::NoEntry => Value::Enum(20),
            filesystem::types::ErrorCode::NoLock => Value::Enum(21),
            filesystem::types::ErrorCode::InsufficientMemory => Value::Enum(22),
            filesystem::types::ErrorCode::InsufficientSpace => Value::Enum(23),
            filesystem::types::ErrorCode::NotDirectory => Value::Enum(24),
            filesystem::types::ErrorCode::NotEmpty => Value::Enum(25),
            filesystem::types::ErrorCode::NotRecoverable => Value::Enum(26),
            filesystem::types::ErrorCode::Unsupported => Value::Enum(27),
            filesystem::types::ErrorCode::NoTty => Value::Enum(28),
            filesystem::types::ErrorCode::NoSuchDevice => Value::Enum(29),
            filesystem::types::ErrorCode::Overflow => Value::Enum(30),
            filesystem::types::ErrorCode::NotPermitted => Value::Enum(31),
            filesystem::types::ErrorCode::Pipe => Value::Enum(32),
            filesystem::types::ErrorCode::ReadOnly => Value::Enum(33),
            filesystem::types::ErrorCode::InvalidSeek => Value::Enum(34),
            filesystem::types::ErrorCode::TextFileBusy => Value::Enum(35),
            filesystem::types::ErrorCode::CrossDevice => Value::Enum(36),
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
            Value::Enum(0) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::Access,
            )),
            Value::Enum(1) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::WouldBlock,
            )),
            Value::Enum(2) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::Already,
            )),
            Value::Enum(3) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::BadDescriptor,
            )),
            Value::Enum(4) => Ok(SerializableFsErrorCode(filesystem::types::ErrorCode::Busy)),
            Value::Enum(5) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::Deadlock,
            )),
            Value::Enum(6) => Ok(SerializableFsErrorCode(filesystem::types::ErrorCode::Quota)),
            Value::Enum(7) => Ok(SerializableFsErrorCode(filesystem::types::ErrorCode::Exist)),
            Value::Enum(8) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::FileTooLarge,
            )),
            Value::Enum(9) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::IllegalByteSequence,
            )),
            Value::Enum(10) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::InProgress,
            )),
            Value::Enum(11) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::Interrupted,
            )),
            Value::Enum(12) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::Invalid,
            )),
            Value::Enum(13) => Ok(SerializableFsErrorCode(filesystem::types::ErrorCode::Io)),
            Value::Enum(14) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::IsDirectory,
            )),
            Value::Enum(15) => Ok(SerializableFsErrorCode(filesystem::types::ErrorCode::Loop)),
            Value::Enum(16) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::TooManyLinks,
            )),
            Value::Enum(17) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::MessageSize,
            )),
            Value::Enum(18) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::NameTooLong,
            )),
            Value::Enum(19) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::NoDevice,
            )),
            Value::Enum(20) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::NoEntry,
            )),
            Value::Enum(21) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::NoLock,
            )),
            Value::Enum(22) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::InsufficientMemory,
            )),
            Value::Enum(23) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::InsufficientSpace,
            )),
            Value::Enum(24) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::NotDirectory,
            )),
            Value::Enum(25) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::NotEmpty,
            )),
            Value::Enum(26) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::NotRecoverable,
            )),
            Value::Enum(27) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::Unsupported,
            )),
            Value::Enum(28) => Ok(SerializableFsErrorCode(filesystem::types::ErrorCode::NoTty)),
            Value::Enum(29) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::NoSuchDevice,
            )),
            Value::Enum(30) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::Overflow,
            )),
            Value::Enum(31) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::NotPermitted,
            )),
            Value::Enum(32) => Ok(SerializableFsErrorCode(filesystem::types::ErrorCode::Pipe)),
            Value::Enum(33) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::ReadOnly,
            )),
            Value::Enum(34) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::InvalidSeek,
            )),
            Value::Enum(35) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::TextFileBusy,
            )),
            Value::Enum(36) => Ok(SerializableFsErrorCode(
                filesystem::types::ErrorCode::CrossDevice,
            )),
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
    pub fn from_result(result: Result<SocketErrorCode, String>) -> Self {
        match result {
            Ok(error_code) => Self::ErrorCode(SerializableSocketErrorCode(error_code)),
            Err(msg) => SerializableSocketError::Generic(msg),
        }
    }
}

impl From<wasmtime_wasi::p2::SocketError> for SerializableSocketError {
    fn from(value: SocketError) -> Self {
        Self::from_result(value.downcast().map_err(|err| err.to_string()))
    }
}

impl From<SerializableSocketError> for wasmtime_wasi::p2::SocketError {
    fn from(value: SerializableSocketError) -> Self {
        match value {
            SerializableSocketError::ErrorCode(SerializableSocketErrorCode(error_code)) => {
                error_code.into()
            }
            SerializableSocketError::Generic(error) => {
                wasmtime_wasi::p2::SocketError::trap(anyhow!(error))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerializableSocketErrorCode(SocketErrorCode);

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
                )))
            }
        };
        Ok(SerializableSocketErrorCode(error_code))
    }
}

impl IntoValue for SerializableSocketErrorCode {
    fn into_value(self) -> Value {
        match &self.0 {
            SocketErrorCode::Unknown => Value::Enum(0),
            SocketErrorCode::AccessDenied => Value::Enum(1),
            SocketErrorCode::NotSupported => Value::Enum(2),
            SocketErrorCode::InvalidArgument => Value::Enum(3),
            SocketErrorCode::OutOfMemory => Value::Enum(4),
            SocketErrorCode::Timeout => Value::Enum(5),
            SocketErrorCode::ConcurrencyConflict => Value::Enum(6),
            SocketErrorCode::NotInProgress => Value::Enum(7),
            SocketErrorCode::WouldBlock => Value::Enum(8),
            SocketErrorCode::InvalidState => Value::Enum(9),
            SocketErrorCode::NewSocketLimit => Value::Enum(10),
            SocketErrorCode::AddressNotBindable => Value::Enum(11),
            SocketErrorCode::AddressInUse => Value::Enum(12),
            SocketErrorCode::RemoteUnreachable => Value::Enum(13),
            SocketErrorCode::ConnectionRefused => Value::Enum(14),
            SocketErrorCode::ConnectionReset => Value::Enum(15),
            SocketErrorCode::ConnectionAborted => Value::Enum(16),
            SocketErrorCode::DatagramTooLarge => Value::Enum(17),
            SocketErrorCode::NameUnresolvable => Value::Enum(18),
            SocketErrorCode::TemporaryResolverFailure => Value::Enum(19),
            SocketErrorCode::PermanentResolverFailure => Value::Enum(20),
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
            Value::Enum(0) => Ok(SerializableSocketErrorCode(SocketErrorCode::Unknown)),
            Value::Enum(1) => Ok(SerializableSocketErrorCode(SocketErrorCode::AccessDenied)),
            Value::Enum(2) => Ok(SerializableSocketErrorCode(SocketErrorCode::NotSupported)),
            Value::Enum(3) => Ok(SerializableSocketErrorCode(
                SocketErrorCode::InvalidArgument,
            )),
            Value::Enum(4) => Ok(SerializableSocketErrorCode(SocketErrorCode::OutOfMemory)),
            Value::Enum(5) => Ok(SerializableSocketErrorCode(SocketErrorCode::Timeout)),
            Value::Enum(6) => Ok(SerializableSocketErrorCode(
                SocketErrorCode::ConcurrencyConflict,
            )),
            Value::Enum(7) => Ok(SerializableSocketErrorCode(SocketErrorCode::NotInProgress)),
            Value::Enum(8) => Ok(SerializableSocketErrorCode(SocketErrorCode::WouldBlock)),
            Value::Enum(9) => Ok(SerializableSocketErrorCode(SocketErrorCode::InvalidState)),
            Value::Enum(10) => Ok(SerializableSocketErrorCode(SocketErrorCode::NewSocketLimit)),
            Value::Enum(11) => Ok(SerializableSocketErrorCode(
                SocketErrorCode::AddressNotBindable,
            )),
            Value::Enum(12) => Ok(SerializableSocketErrorCode(SocketErrorCode::AddressInUse)),
            Value::Enum(13) => Ok(SerializableSocketErrorCode(
                SocketErrorCode::RemoteUnreachable,
            )),
            Value::Enum(14) => Ok(SerializableSocketErrorCode(
                SocketErrorCode::ConnectionRefused,
            )),
            Value::Enum(15) => Ok(SerializableSocketErrorCode(
                SocketErrorCode::ConnectionReset,
            )),
            Value::Enum(16) => Ok(SerializableSocketErrorCode(
                SocketErrorCode::ConnectionAborted,
            )),
            Value::Enum(17) => Ok(SerializableSocketErrorCode(
                SocketErrorCode::DatagramTooLarge,
            )),
            Value::Enum(18) => Ok(SerializableSocketErrorCode(
                SocketErrorCode::NameUnresolvable,
            )),
            Value::Enum(19) => Ok(SerializableSocketErrorCode(
                SocketErrorCode::TemporaryResolverFailure,
            )),
            Value::Enum(20) => Ok(SerializableSocketErrorCode(
                SocketErrorCode::PermanentResolverFailure,
            )),
            _ => Err(format!(
                "Invalid value for SerializableSocketErrorCode: {:?}",
                value
            )),
        }
    }
}

#[derive(Debug, Clone, BinaryCodec)]
pub enum SerializedHttpVersion {
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

impl TryFrom<Version> for SerializedHttpVersion {
    type Error = String;

    fn try_from(value: Version) -> Result<Self, Self::Error> {
        if value == Version::HTTP_09 {
            Ok(SerializedHttpVersion::Http09)
        } else if value == Version::HTTP_10 {
            Ok(SerializedHttpVersion::Http10)
        } else if value == Version::HTTP_11 {
            Ok(SerializedHttpVersion::Http11)
        } else if value == Version::HTTP_2 {
            Ok(SerializedHttpVersion::Http2)
        } else if value == Version::HTTP_3 {
            Ok(SerializedHttpVersion::Http3)
        } else {
            Err(format!("Unknown HTTP version: {value:?}"))
        }
    }
}

impl From<SerializedHttpVersion> for Version {
    fn from(value: SerializedHttpVersion) -> Self {
        match value {
            SerializedHttpVersion::Http09 => Version::HTTP_09,
            SerializedHttpVersion::Http10 => Version::HTTP_10,
            SerializedHttpVersion::Http11 => Version::HTTP_11,
            SerializedHttpVersion::Http2 => Version::HTTP_2,
            SerializedHttpVersion::Http3 => Version::HTTP_3,
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
    pub headers: HashMap<String, Vec<u8>>,
}

impl TryFrom<&HostIncomingResponse> for SerializableResponseHeaders {
    type Error = anyhow::Error;

    fn try_from(response: &HostIncomingResponse) -> Result<Self, Self::Error> {
        let mut headers = HashMap::new();
        for (key, value) in response.headers.iter() {
            headers.insert(key.as_str().to_string(), value.as_bytes().to_vec());
        }

        Ok(Self {
            status: response.status,
            headers,
        })
    }
}

impl TryFrom<SerializableResponseHeaders> for HostIncomingResponse {
    type Error = anyhow::Error;

    fn try_from(value: SerializableResponseHeaders) -> Result<Self, Self::Error> {
        let mut headers = FieldMap::new();
        for (key, value) in value.headers {
            headers.insert(HeaderName::from_str(&key)?, HeaderValue::try_from(value)?);
        }

        Ok(Self {
            status: value.status,
            headers,
            body: Some(HostIncomingBody::failing(
                "Body stream was interrupted due to a restart".to_string(),
            )), // NOTE: high enough timeout so it does not matter, but not as high to overflow instants
        })
    }
}

#[derive(Debug, Clone, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub struct SerializableTlsAlertReceivedPayload {
    pub alert_id: Option<u8>,
    pub alert_message: Option<String>,
}

impl From<&TlsAlertReceivedPayload> for SerializableTlsAlertReceivedPayload {
    fn from(value: &TlsAlertReceivedPayload) -> Self {
        Self {
            alert_id: value.alert_id,
            alert_message: value.alert_message.clone(),
        }
    }
}

impl From<SerializableTlsAlertReceivedPayload> for TlsAlertReceivedPayload {
    fn from(value: SerializableTlsAlertReceivedPayload) -> Self {
        Self {
            alert_id: value.alert_id,
            alert_message: value.alert_message,
        }
    }
}

#[derive(Debug, Clone, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub struct SerializableDnsErrorPayload {
    pub rcode: Option<String>,
    pub info_code: Option<u16>,
}

impl From<&DnsErrorPayload> for SerializableDnsErrorPayload {
    fn from(value: &DnsErrorPayload) -> Self {
        Self {
            rcode: value.rcode.clone(),
            info_code: value.info_code,
        }
    }
}

impl From<SerializableDnsErrorPayload> for DnsErrorPayload {
    fn from(value: SerializableDnsErrorPayload) -> Self {
        Self {
            rcode: value.rcode,
            info_code: value.info_code,
        }
    }
}

#[derive(Debug, Clone, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub struct SerializableFieldSizePayload {
    pub field_name: Option<String>,
    pub field_size: Option<u32>,
}

impl From<&FieldSizePayload> for SerializableFieldSizePayload {
    fn from(value: &FieldSizePayload) -> Self {
        Self {
            field_name: value.field_name.clone(),
            field_size: value.field_size,
        }
    }
}

impl From<SerializableFieldSizePayload> for FieldSizePayload {
    fn from(value: SerializableFieldSizePayload) -> Self {
        Self {
            field_name: value.field_name,
            field_size: value.field_size,
        }
    }
}

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

impl From<wasmtime_wasi_http::bindings::http::types::ErrorCode> for SerializableHttpErrorCode {
    fn from(value: wasmtime_wasi_http::bindings::http::types::ErrorCode) -> Self {
        (&value).into()
    }
}

impl From<&wasmtime_wasi_http::bindings::http::types::ErrorCode> for SerializableHttpErrorCode {
    fn from(value: &wasmtime_wasi_http::bindings::http::types::ErrorCode) -> Self {
        use wasmtime_wasi_http::bindings::http::types::ErrorCode;

        match value {
            ErrorCode::DnsTimeout => SerializableHttpErrorCode::DnsTimeout,
            ErrorCode::DnsError(payload) => SerializableHttpErrorCode::DnsError(payload.into()),
            ErrorCode::DestinationNotFound => SerializableHttpErrorCode::DestinationNotFound,
            ErrorCode::DestinationUnavailable => SerializableHttpErrorCode::DestinationUnavailable,
            ErrorCode::DestinationIpProhibited => {
                SerializableHttpErrorCode::DestinationIpProhibited
            }
            ErrorCode::DestinationIpUnroutable => {
                SerializableHttpErrorCode::DestinationIpUnroutable
            }
            ErrorCode::ConnectionRefused => SerializableHttpErrorCode::ConnectionRefused,
            ErrorCode::ConnectionTerminated => SerializableHttpErrorCode::ConnectionTerminated,
            ErrorCode::ConnectionTimeout => SerializableHttpErrorCode::ConnectionTimeout,
            ErrorCode::ConnectionReadTimeout => SerializableHttpErrorCode::ConnectionReadTimeout,
            ErrorCode::ConnectionWriteTimeout => SerializableHttpErrorCode::ConnectionWriteTimeout,
            ErrorCode::ConnectionLimitReached => SerializableHttpErrorCode::ConnectionLimitReached,
            ErrorCode::TlsProtocolError => SerializableHttpErrorCode::TlsProtocolError,
            ErrorCode::TlsCertificateError => SerializableHttpErrorCode::TlsCertificateError,
            ErrorCode::TlsAlertReceived(payload) => {
                SerializableHttpErrorCode::TlsAlertReceived(payload.into())
            }
            ErrorCode::HttpRequestDenied => SerializableHttpErrorCode::HttpRequestDenied,
            ErrorCode::HttpRequestLengthRequired => {
                SerializableHttpErrorCode::HttpRequestLengthRequired
            }
            ErrorCode::HttpRequestBodySize(payload) => {
                SerializableHttpErrorCode::HttpRequestBodySize(*payload)
            }
            ErrorCode::HttpRequestMethodInvalid => {
                SerializableHttpErrorCode::HttpRequestMethodInvalid
            }
            ErrorCode::HttpRequestUriInvalid => SerializableHttpErrorCode::HttpRequestUriInvalid,
            ErrorCode::HttpRequestUriTooLong => SerializableHttpErrorCode::HttpRequestUriTooLong,
            ErrorCode::HttpRequestHeaderSectionSize(payload) => {
                SerializableHttpErrorCode::HttpRequestHeaderSectionSize(*payload)
            }
            ErrorCode::HttpRequestHeaderSize(payload) => {
                SerializableHttpErrorCode::HttpRequestHeaderSize(payload.as_ref().map(|p| p.into()))
            }
            ErrorCode::HttpRequestTrailerSectionSize(payload) => {
                SerializableHttpErrorCode::HttpRequestTrailerSectionSize(*payload)
            }
            ErrorCode::HttpRequestTrailerSize(payload) => {
                SerializableHttpErrorCode::HttpRequestTrailerSize(payload.into())
            }
            ErrorCode::HttpResponseIncomplete => SerializableHttpErrorCode::HttpResponseIncomplete,
            ErrorCode::HttpResponseHeaderSectionSize(payload) => {
                SerializableHttpErrorCode::HttpResponseHeaderSectionSize(*payload)
            }
            ErrorCode::HttpResponseHeaderSize(payload) => {
                SerializableHttpErrorCode::HttpResponseHeaderSize(payload.into())
            }
            ErrorCode::HttpResponseBodySize(payload) => {
                SerializableHttpErrorCode::HttpResponseBodySize(*payload)
            }
            ErrorCode::HttpResponseTrailerSectionSize(payload) => {
                SerializableHttpErrorCode::HttpResponseTrailerSectionSize(*payload)
            }
            ErrorCode::HttpResponseTrailerSize(payload) => {
                SerializableHttpErrorCode::HttpResponseTrailerSize(payload.into())
            }
            ErrorCode::HttpResponseTransferCoding(payload) => {
                SerializableHttpErrorCode::HttpResponseTransferCoding(payload.clone())
            }
            ErrorCode::HttpResponseContentCoding(payload) => {
                SerializableHttpErrorCode::HttpResponseContentCoding(payload.clone())
            }
            ErrorCode::HttpResponseTimeout => SerializableHttpErrorCode::HttpResponseTimeout,
            ErrorCode::HttpUpgradeFailed => SerializableHttpErrorCode::HttpUpgradeFailed,
            ErrorCode::HttpProtocolError => SerializableHttpErrorCode::HttpProtocolError,
            ErrorCode::LoopDetected => SerializableHttpErrorCode::LoopDetected,
            ErrorCode::ConfigurationError => SerializableHttpErrorCode::ConfigurationError,
            ErrorCode::InternalError(payload) => {
                SerializableHttpErrorCode::InternalError(payload.clone())
            }
        }
    }
}

impl From<SerializableHttpErrorCode> for wasmtime_wasi_http::bindings::http::types::ErrorCode {
    fn from(value: SerializableHttpErrorCode) -> Self {
        use wasmtime_wasi_http::bindings::http::types::ErrorCode;

        match value {
            SerializableHttpErrorCode::DnsTimeout => ErrorCode::DnsTimeout,
            SerializableHttpErrorCode::DnsError(payload) => ErrorCode::DnsError(payload.into()),
            SerializableHttpErrorCode::DestinationNotFound => ErrorCode::DestinationNotFound,
            SerializableHttpErrorCode::DestinationUnavailable => ErrorCode::DestinationUnavailable,
            SerializableHttpErrorCode::DestinationIpProhibited => {
                ErrorCode::DestinationIpProhibited
            }
            SerializableHttpErrorCode::DestinationIpUnroutable => {
                ErrorCode::DestinationIpUnroutable
            }
            SerializableHttpErrorCode::ConnectionRefused => ErrorCode::ConnectionRefused,
            SerializableHttpErrorCode::ConnectionTerminated => ErrorCode::ConnectionTerminated,
            SerializableHttpErrorCode::ConnectionTimeout => ErrorCode::ConnectionTimeout,
            SerializableHttpErrorCode::ConnectionReadTimeout => ErrorCode::ConnectionReadTimeout,
            SerializableHttpErrorCode::ConnectionWriteTimeout => ErrorCode::ConnectionWriteTimeout,
            SerializableHttpErrorCode::ConnectionLimitReached => ErrorCode::ConnectionLimitReached,
            SerializableHttpErrorCode::TlsProtocolError => ErrorCode::TlsProtocolError,
            SerializableHttpErrorCode::TlsCertificateError => ErrorCode::TlsCertificateError,
            SerializableHttpErrorCode::TlsAlertReceived(payload) => {
                ErrorCode::TlsAlertReceived(payload.into())
            }
            SerializableHttpErrorCode::HttpRequestDenied => ErrorCode::HttpRequestDenied,
            SerializableHttpErrorCode::HttpRequestLengthRequired => {
                ErrorCode::HttpRequestLengthRequired
            }
            SerializableHttpErrorCode::HttpRequestBodySize(payload) => {
                ErrorCode::HttpRequestBodySize(payload)
            }
            SerializableHttpErrorCode::HttpRequestMethodInvalid => {
                ErrorCode::HttpRequestMethodInvalid
            }
            SerializableHttpErrorCode::HttpRequestUriInvalid => ErrorCode::HttpRequestUriInvalid,
            SerializableHttpErrorCode::HttpRequestUriTooLong => ErrorCode::HttpRequestUriTooLong,
            SerializableHttpErrorCode::HttpRequestHeaderSectionSize(payload) => {
                ErrorCode::HttpRequestHeaderSectionSize(payload)
            }
            SerializableHttpErrorCode::HttpRequestHeaderSize(payload) => {
                ErrorCode::HttpRequestHeaderSize(payload.map(|p| p.into()))
            }
            SerializableHttpErrorCode::HttpRequestTrailerSectionSize(payload) => {
                ErrorCode::HttpRequestTrailerSectionSize(payload)
            }
            SerializableHttpErrorCode::HttpRequestTrailerSize(payload) => {
                ErrorCode::HttpRequestTrailerSize(payload.into())
            }
            SerializableHttpErrorCode::HttpResponseIncomplete => ErrorCode::HttpResponseIncomplete,
            SerializableHttpErrorCode::HttpResponseHeaderSectionSize(payload) => {
                ErrorCode::HttpResponseHeaderSectionSize(payload)
            }
            SerializableHttpErrorCode::HttpResponseHeaderSize(payload) => {
                ErrorCode::HttpResponseHeaderSize(payload.into())
            }
            SerializableHttpErrorCode::HttpResponseBodySize(payload) => {
                ErrorCode::HttpResponseBodySize(payload)
            }
            SerializableHttpErrorCode::HttpResponseTrailerSectionSize(payload) => {
                ErrorCode::HttpResponseTrailerSectionSize(payload)
            }
            SerializableHttpErrorCode::HttpResponseTrailerSize(payload) => {
                ErrorCode::HttpResponseTrailerSize(payload.into())
            }
            SerializableHttpErrorCode::HttpResponseTransferCoding(payload) => {
                ErrorCode::HttpResponseTransferCoding(payload)
            }
            SerializableHttpErrorCode::HttpResponseContentCoding(payload) => {
                ErrorCode::HttpResponseContentCoding(payload)
            }
            SerializableHttpErrorCode::HttpResponseTimeout => ErrorCode::HttpResponseTimeout,
            SerializableHttpErrorCode::HttpUpgradeFailed => ErrorCode::HttpUpgradeFailed,
            SerializableHttpErrorCode::HttpProtocolError => ErrorCode::HttpProtocolError,
            SerializableHttpErrorCode::LoopDetected => ErrorCode::LoopDetected,
            SerializableHttpErrorCode::ConfigurationError => ErrorCode::ConfigurationError,
            SerializableHttpErrorCode::InternalError(payload) => ErrorCode::InternalError(payload),
        }
    }
}

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

impl From<Method> for SerializableHttpMethod {
    fn from(value: Method) -> Self {
        match value {
            Method::Get => SerializableHttpMethod::Get,
            Method::Post => SerializableHttpMethod::Post,
            Method::Put => SerializableHttpMethod::Put,
            Method::Delete => SerializableHttpMethod::Delete,
            Method::Head => SerializableHttpMethod::Head,
            Method::Connect => SerializableHttpMethod::Connect,
            Method::Options => SerializableHttpMethod::Options,
            Method::Trace => SerializableHttpMethod::Trace,
            Method::Patch => SerializableHttpMethod::Patch,
            Method::Other(method) => SerializableHttpMethod::Other(method),
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

#[derive(Debug, Clone, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub struct SerializableHttpRequest {
    pub uri: String,
    pub method: SerializableHttpMethod,
    pub headers: HashMap<String, String>,
}

/// A subset of WorkerMetadata visible for guests (and serializable to oplog)
#[derive(Debug, Clone, PartialEq, IntoValue, FromValue, BinaryCodec)]
pub struct AgentMetadataForGuests {
    pub agent_id: WorkerId,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub config_vars: BTreeMap<String, String>,
    pub status: WorkerStatus,
    pub component_version: ComponentVersion,
    pub retry_count: u64,
}

impl From<WorkerMetadata> for AgentMetadataForGuests {
    fn from(value: WorkerMetadata) -> Self {
        Self {
            agent_id: value.worker_id,
            args: value.args,
            env: value.env,
            config_vars: value.wasi_config_vars,
            status: value.last_known_status.status,
            component_version: value.last_known_status.component_version,
            retry_count: value
                .last_known_status
                .current_retry_count
                .iter()
                .max_by_key(|(idx, _)| **idx)
                .map(|(_, value)| *value)
                .unwrap_or_default() as u64,
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
                Self::LastOperationFailed(anyhow!(e))
            }
            SerializableStreamError::Trap(e) => Self::Trap(anyhow!(e)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
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

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec, IntoValue, FromValue)]
#[desert(transparent)]
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

#[derive(Debug, Clone, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub struct SerializableInvokeRequest {
    pub remote_worker_id: WorkerId,
    pub idempotency_key: IdempotencyKey,
    pub function_name: String,
    pub function_params: Vec<ValueAndType>,
}

#[derive(Debug, Clone, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub enum SerializableInvokeResult {
    Failed(String),
    Pending,
    Completed(Result<Option<ValueAndType>, SerializableRpcError>),
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
pub enum SerializableRpcError {
    ProtocolError { details: String },
    Denied { details: String },
    NotFound { details: String },
    RemoteInternalError { details: String },
}

#[derive(Debug, Clone, PartialEq, BinaryCodec, IntoValue, FromValue)]
#[desert(evolution())]
#[wit_transparent]
pub struct SerializableScheduledInvocation {
    pub timestamp: i64,
    pub account_id: AccountId,
    pub project_id: ProjectId,
    pub worker_id: WorkerId,
    pub idempotency_key: IdempotencyKey,
    pub full_function_name: String,
    pub function_input: Vec<Value>,
    pub trace_id: TraceId,
    pub trace_states: Vec<String>,
    pub spans: Vec<Vec<PublicSpanData>>,
}

impl SerializableScheduledInvocation {
    pub fn from_domain(schedule_id: ScheduleId) -> Result<Self, String> {
        match schedule_id.action {
            ScheduledAction::Invoke {
                account_id,
                owned_worker_id,
                idempotency_key,
                full_function_name,
                function_input,
                invocation_context,
            } => Ok(Self {
                timestamp: schedule_id.timestamp,
                account_id,
                project_id: owned_worker_id.project_id,
                worker_id: owned_worker_id.worker_id,
                idempotency_key,
                full_function_name,
                function_input,
                spans: encode_span_data(&invocation_context.to_oplog_data()),
                trace_id: invocation_context.trace_id,
                trace_states: invocation_context.trace_states,
            }),
            _ => Err("ScheduleId does not describe an invocation".to_string()),
        }
    }

    pub fn into_domain(self) -> ScheduleId {
        ScheduleId {
            timestamp: self.timestamp,
            action: ScheduledAction::Invoke {
                account_id: self.account_id,
                owned_worker_id: OwnedWorkerId {
                    project_id: self.project_id,
                    worker_id: self.worker_id,
                },
                idempotency_key: self.idempotency_key,
                full_function_name: self.full_function_name,
                function_input: self.function_input,
                invocation_context: InvocationContextStack::from_oplog_data(
                    self.trace_id,
                    self.trace_states,
                    decode_span_data(self.spans),
                ),
            },
        }
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
            if let PublicSpanData::LocalSpan(ref mut local_span) = span {
                if let Some(linked_id) = &mut local_span.linked_context {
                    *linked_id += 1;
                }
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
