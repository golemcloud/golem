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
use crate::model::component::ComponentRevision;
use crate::model::environment::EnvironmentId;
use crate::model::invocation_context::AttributeValue;
use crate::model::oplog::{
    PublicAttribute, PublicExternalSpanData, PublicLocalSpanData, PublicSpanData, SpanData,
};
use crate::model::worker::TypedAgentConfigEntry;
use crate::model::{AgentId, AgentMetadata, AgentStatus, RdbmsPoolKey, ScheduleId};
use crate::schema::conversion::{FromSchemaError, SchemaBuilder, value_kind};
use crate::schema::metadata::TypeId;
use crate::schema::schema_type::SchemaType;
use crate::schema::schema_value::SchemaValue;
use bigdecimal::BigDecimal;
use bit_vec::BitVec;
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use desert_rust::{
    BinaryCodec, BinaryDeserializer, BinaryInput, BinaryOutput, BinarySerializer,
    DeserializationContext, SerializationContext,
};
use golem_schema::schema::wit::wire::ValueNodeIndex as NodeIndex;
use http::{HeaderName, HeaderValue, Version};
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
use wasmtime_wasi::p2::bindings::filesystem;
use wasmtime_wasi::p2::bindings::sockets::ip_name_lookup::IpAddress;
use wasmtime_wasi::p2::bindings::sockets::network::ErrorCode as SocketErrorCode;
use wasmtime_wasi::p2::{FsError, SocketError};
use wasmtime_wasi::p3::bindings::cli::types as p3_cli_types;
use wasmtime_wasi::p3::bindings::filesystem as p3_filesystem;
use wasmtime_wasi::p3::bindings::sockets::{
    ip_name_lookup as p3_ip_name_lookup, types as p3_socket_types,
};
use wasmtime_wasi_http::FieldMap;
use wasmtime_wasi_http::p2::bindings::http::types::{
    DnsErrorPayload, FieldSizePayload, Method, TlsAlertReceivedPayload,
};
use wasmtime_wasi_http::p2::body::HostIncomingBody;
use wasmtime_wasi_http::p2::types::HostIncomingResponse;

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub struct ObjectMetadata {
    pub name: String,
    pub container: String,
    pub created_at: u64,
    pub size: u64,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub struct SerializableDateTime {
    pub seconds: i64,
    pub nanoseconds: u32,
}

impl From<wasmtime_wasi::p3::bindings::clocks::system_clock::Instant> for SerializableDateTime {
    fn from(value: wasmtime_wasi::p3::bindings::clocks::system_clock::Instant) -> Self {
        Self {
            seconds: value.seconds,
            nanoseconds: value.nanoseconds,
        }
    }
}

impl From<SerializableDateTime> for wasmtime_wasi::p3::bindings::clocks::system_clock::Instant {
    fn from(value: SerializableDateTime) -> Self {
        Self {
            seconds: value.seconds,
            nanoseconds: value.nanoseconds,
        }
    }
}

impl From<wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime> for SerializableDateTime {
    fn from(value: wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime) -> Self {
        Self {
            seconds: value.seconds as i64,
            nanoseconds: value.nanoseconds,
        }
    }
}

impl From<SerializableDateTime> for wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime {
    fn from(value: SerializableDateTime) -> Self {
        Self {
            seconds: value.seconds.max(0) as u64,
            nanoseconds: value.nanoseconds,
        }
    }
}

impl From<SerializableDateTime> for SystemTime {
    fn from(value: SerializableDateTime) -> Self {
        SystemTime::UNIX_EPOCH.add(Duration::new(
            value.seconds.max(0) as u64,
            value.nanoseconds,
        ))
    }
}

impl From<SystemTime> for SerializableDateTime {
    fn from(value: SystemTime) -> Self {
        let duration = value.duration_since(SystemTime::UNIX_EPOCH).unwrap();
        Self {
            seconds: duration.as_secs() as i64,
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

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub struct SerializableFileTimes {
    pub data_access_timestamp: Option<SerializableDateTime>,
    pub data_modification_timestamp: Option<SerializableDateTime>,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
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
            FileSystemError::Generic(error) => FsError::trap(wasmtime::Error::msg(error)),
        }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub enum SerializableP3FileSystemError {
    ErrorCode(SerializableP3FsErrorCode),
    Generic(String),
}

impl SerializableP3FileSystemError {
    pub fn from_result(result: Result<p3_filesystem::types::ErrorCode, String>) -> Self {
        match result {
            Ok(error_code) => Self::ErrorCode(SerializableP3FsErrorCode::from(error_code)),
            Err(msg) => SerializableP3FileSystemError::Generic(msg),
        }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub enum SerializableP3CliErrorCode {
    Io,
    IllegalByteSequence,
    Pipe,
}

impl From<p3_cli_types::ErrorCode> for SerializableP3CliErrorCode {
    fn from(value: p3_cli_types::ErrorCode) -> Self {
        match value {
            p3_cli_types::ErrorCode::IllegalByteSequence => Self::IllegalByteSequence,
            p3_cli_types::ErrorCode::Pipe => Self::Pipe,
            _ => Self::Io,
        }
    }
}

impl From<SerializableP3CliErrorCode> for p3_cli_types::ErrorCode {
    fn from(value: SerializableP3CliErrorCode) -> Self {
        match value {
            SerializableP3CliErrorCode::Io => Self::Io,
            SerializableP3CliErrorCode::IllegalByteSequence => Self::IllegalByteSequence,
            SerializableP3CliErrorCode::Pipe => Self::Pipe,
        }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub enum SerializableP3FsErrorCode {
    Access,
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
    Other(Option<String>),
}

impl From<p3_filesystem::types::ErrorCode> for SerializableP3FsErrorCode {
    fn from(value: p3_filesystem::types::ErrorCode) -> Self {
        match value {
            p3_filesystem::types::ErrorCode::Access => Self::Access,
            p3_filesystem::types::ErrorCode::Already => Self::Already,
            p3_filesystem::types::ErrorCode::BadDescriptor => Self::BadDescriptor,
            p3_filesystem::types::ErrorCode::Busy => Self::Busy,
            p3_filesystem::types::ErrorCode::Deadlock => Self::Deadlock,
            p3_filesystem::types::ErrorCode::Quota => Self::Quota,
            p3_filesystem::types::ErrorCode::Exist => Self::Exist,
            p3_filesystem::types::ErrorCode::FileTooLarge => Self::FileTooLarge,
            p3_filesystem::types::ErrorCode::IllegalByteSequence => Self::IllegalByteSequence,
            p3_filesystem::types::ErrorCode::InProgress => Self::InProgress,
            p3_filesystem::types::ErrorCode::Interrupted => Self::Interrupted,
            p3_filesystem::types::ErrorCode::Invalid => Self::Invalid,
            p3_filesystem::types::ErrorCode::Io => Self::Io,
            p3_filesystem::types::ErrorCode::IsDirectory => Self::IsDirectory,
            p3_filesystem::types::ErrorCode::Loop => Self::Loop,
            p3_filesystem::types::ErrorCode::TooManyLinks => Self::TooManyLinks,
            p3_filesystem::types::ErrorCode::MessageSize => Self::MessageSize,
            p3_filesystem::types::ErrorCode::NameTooLong => Self::NameTooLong,
            p3_filesystem::types::ErrorCode::NoDevice => Self::NoDevice,
            p3_filesystem::types::ErrorCode::NoEntry => Self::NoEntry,
            p3_filesystem::types::ErrorCode::NoLock => Self::NoLock,
            p3_filesystem::types::ErrorCode::InsufficientMemory => Self::InsufficientMemory,
            p3_filesystem::types::ErrorCode::InsufficientSpace => Self::InsufficientSpace,
            p3_filesystem::types::ErrorCode::NotDirectory => Self::NotDirectory,
            p3_filesystem::types::ErrorCode::NotEmpty => Self::NotEmpty,
            p3_filesystem::types::ErrorCode::NotRecoverable => Self::NotRecoverable,
            p3_filesystem::types::ErrorCode::Unsupported => Self::Unsupported,
            p3_filesystem::types::ErrorCode::NoTty => Self::NoTty,
            p3_filesystem::types::ErrorCode::NoSuchDevice => Self::NoSuchDevice,
            p3_filesystem::types::ErrorCode::Overflow => Self::Overflow,
            p3_filesystem::types::ErrorCode::NotPermitted => Self::NotPermitted,
            p3_filesystem::types::ErrorCode::Pipe => Self::Pipe,
            p3_filesystem::types::ErrorCode::ReadOnly => Self::ReadOnly,
            p3_filesystem::types::ErrorCode::InvalidSeek => Self::InvalidSeek,
            p3_filesystem::types::ErrorCode::TextFileBusy => Self::TextFileBusy,
            p3_filesystem::types::ErrorCode::CrossDevice => Self::CrossDevice,
            p3_filesystem::types::ErrorCode::Other(error) => Self::Other(error),
        }
    }
}

impl From<SerializableP3FsErrorCode> for p3_filesystem::types::ErrorCode {
    fn from(value: SerializableP3FsErrorCode) -> Self {
        match value {
            SerializableP3FsErrorCode::Access => Self::Access,
            SerializableP3FsErrorCode::Already => Self::Already,
            SerializableP3FsErrorCode::BadDescriptor => Self::BadDescriptor,
            SerializableP3FsErrorCode::Busy => Self::Busy,
            SerializableP3FsErrorCode::Deadlock => Self::Deadlock,
            SerializableP3FsErrorCode::Quota => Self::Quota,
            SerializableP3FsErrorCode::Exist => Self::Exist,
            SerializableP3FsErrorCode::FileTooLarge => Self::FileTooLarge,
            SerializableP3FsErrorCode::IllegalByteSequence => Self::IllegalByteSequence,
            SerializableP3FsErrorCode::InProgress => Self::InProgress,
            SerializableP3FsErrorCode::Interrupted => Self::Interrupted,
            SerializableP3FsErrorCode::Invalid => Self::Invalid,
            SerializableP3FsErrorCode::Io => Self::Io,
            SerializableP3FsErrorCode::IsDirectory => Self::IsDirectory,
            SerializableP3FsErrorCode::Loop => Self::Loop,
            SerializableP3FsErrorCode::TooManyLinks => Self::TooManyLinks,
            SerializableP3FsErrorCode::MessageSize => Self::MessageSize,
            SerializableP3FsErrorCode::NameTooLong => Self::NameTooLong,
            SerializableP3FsErrorCode::NoDevice => Self::NoDevice,
            SerializableP3FsErrorCode::NoEntry => Self::NoEntry,
            SerializableP3FsErrorCode::NoLock => Self::NoLock,
            SerializableP3FsErrorCode::InsufficientMemory => Self::InsufficientMemory,
            SerializableP3FsErrorCode::InsufficientSpace => Self::InsufficientSpace,
            SerializableP3FsErrorCode::NotDirectory => Self::NotDirectory,
            SerializableP3FsErrorCode::NotEmpty => Self::NotEmpty,
            SerializableP3FsErrorCode::NotRecoverable => Self::NotRecoverable,
            SerializableP3FsErrorCode::Unsupported => Self::Unsupported,
            SerializableP3FsErrorCode::NoTty => Self::NoTty,
            SerializableP3FsErrorCode::NoSuchDevice => Self::NoSuchDevice,
            SerializableP3FsErrorCode::Overflow => Self::Overflow,
            SerializableP3FsErrorCode::NotPermitted => Self::NotPermitted,
            SerializableP3FsErrorCode::Pipe => Self::Pipe,
            SerializableP3FsErrorCode::ReadOnly => Self::ReadOnly,
            SerializableP3FsErrorCode::InvalidSeek => Self::InvalidSeek,
            SerializableP3FsErrorCode::TextFileBusy => Self::TextFileBusy,
            SerializableP3FsErrorCode::CrossDevice => Self::CrossDevice,
            SerializableP3FsErrorCode::Other(error) => Self::Other(error),
        }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub enum SerializableP3DescriptorType {
    BlockDevice,
    CharacterDevice,
    Directory,
    Fifo,
    SymbolicLink,
    RegularFile,
    Socket,
    Other(Option<String>),
}

impl From<p3_filesystem::types::DescriptorType> for SerializableP3DescriptorType {
    fn from(value: p3_filesystem::types::DescriptorType) -> Self {
        match value {
            p3_filesystem::types::DescriptorType::BlockDevice => Self::BlockDevice,
            p3_filesystem::types::DescriptorType::CharacterDevice => Self::CharacterDevice,
            p3_filesystem::types::DescriptorType::Directory => Self::Directory,
            p3_filesystem::types::DescriptorType::Fifo => Self::Fifo,
            p3_filesystem::types::DescriptorType::SymbolicLink => Self::SymbolicLink,
            p3_filesystem::types::DescriptorType::RegularFile => Self::RegularFile,
            p3_filesystem::types::DescriptorType::Socket => Self::Socket,
            p3_filesystem::types::DescriptorType::Other(other) => Self::Other(other),
        }
    }
}

impl From<SerializableP3DescriptorType> for p3_filesystem::types::DescriptorType {
    fn from(value: SerializableP3DescriptorType) -> Self {
        match value {
            SerializableP3DescriptorType::BlockDevice => Self::BlockDevice,
            SerializableP3DescriptorType::CharacterDevice => Self::CharacterDevice,
            SerializableP3DescriptorType::Directory => Self::Directory,
            SerializableP3DescriptorType::Fifo => Self::Fifo,
            SerializableP3DescriptorType::SymbolicLink => Self::SymbolicLink,
            SerializableP3DescriptorType::RegularFile => Self::RegularFile,
            SerializableP3DescriptorType::Socket => Self::Socket,
            SerializableP3DescriptorType::Other(other) => Self::Other(other),
        }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub struct SerializableP3DirectoryEntry {
    pub type_: SerializableP3DescriptorType,
    pub name: String,
}

impl From<p3_filesystem::types::DirectoryEntry> for SerializableP3DirectoryEntry {
    fn from(value: p3_filesystem::types::DirectoryEntry) -> Self {
        Self {
            type_: value.type_.into(),
            name: value.name,
        }
    }
}

impl From<SerializableP3DirectoryEntry> for p3_filesystem::types::DirectoryEntry {
    fn from(value: SerializableP3DirectoryEntry) -> Self {
        Self {
            type_: value.type_.into(),
            name: value.name,
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
                )));
            }
        };
        Ok(SerializableFsErrorCode(error_code))
    }
}

// Schema-native A2 impl: a flat enum mirroring the legacy schema
// above: a flat enum with the same 37 cases/indices.
impl crate::schema::conversion::IntoSchema for SerializableFsErrorCode {
    fn type_id() -> TypeId {
        TypeId::new("golem_common.model.oplog.payload.SerializableFsErrorCode")
    }
    fn register_in(_b: &mut SchemaBuilder) -> SchemaType {
        SchemaType::r#enum(vec![
            "access".to_string(),
            "would-block".to_string(),
            "already".to_string(),
            "bad-descriptor".to_string(),
            "busy".to_string(),
            "deadlock".to_string(),
            "quota".to_string(),
            "exist".to_string(),
            "file-too-large".to_string(),
            "illegal-byte-sequence".to_string(),
            "in-progress".to_string(),
            "interrupted".to_string(),
            "invalid".to_string(),
            "io".to_string(),
            "is-directory".to_string(),
            "loop".to_string(),
            "too-many-links".to_string(),
            "message-size".to_string(),
            "name-too-long".to_string(),
            "no-device".to_string(),
            "no-entry".to_string(),
            "no-lock".to_string(),
            "insufficient-memory".to_string(),
            "insufficient-space".to_string(),
            "not-directory".to_string(),
            "not-empty".to_string(),
            "not-recoverable".to_string(),
            "unsupported".to_string(),
            "no-tty".to_string(),
            "no-such-device".to_string(),
            "overflow".to_string(),
            "not-permitted".to_string(),
            "pipe".to_string(),
            "read-only".to_string(),
            "invalid-seek".to_string(),
            "text-file-busy".to_string(),
            "cross-device".to_string(),
        ])
    }
    fn to_value(&self) -> SchemaValue {
        let case = match &self.0 {
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
        };
        SchemaValue::Enum { case }
    }
}

impl crate::schema::conversion::FromSchema for SerializableFsErrorCode {
    fn from_value(v: &SchemaValue) -> Result<Self, FromSchemaError> {
        let case = match v {
            SchemaValue::Enum { case } => *case,
            other => {
                return Err(FromSchemaError::shape_mismatch(
                    "enum",
                    value_kind(other),
                    "SerializableFsErrorCode",
                ));
            }
        };
        let error_code = match case {
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
                return Err(FromSchemaError::out_of_range(
                    other,
                    37,
                    "SerializableFsErrorCode",
                ));
            }
        };
        Ok(SerializableFsErrorCode(error_code))
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
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
                )));
            }
        };
        Ok(SerializableSocketErrorCode(error_code))
    }
}

// Schema-native A2 impl: a flat enum mirroring the legacy schema
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

#[derive(
    Debug,
    Clone,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub enum SerializableHttpResponse {
    Pending,
    HeadersReceived(SerializableResponseHeaders),
    HttpError(SerializableHttpErrorCode),
    InternalError(Option<String>),
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub struct SerializableResponseHeaders {
    pub status: u16,
    pub headers: HashMap<String, Vec<Vec<u8>>>,
}

impl TryFrom<&HostIncomingResponse> for SerializableResponseHeaders {
    type Error = anyhow::Error;

    fn try_from(response: &HostIncomingResponse) -> Result<Self, Self::Error> {
        let mut headers: HashMap<String, Vec<Vec<u8>>> = HashMap::new();
        for (key, value) in response.headers.iter() {
            headers
                .entry(key.as_str().to_string())
                .or_default()
                .push(value.as_bytes().to_vec());
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
        let mut header_map = http::HeaderMap::new();
        for (key, values) in value.headers {
            let name = HeaderName::from_str(&key)?;
            for value in values {
                header_map.append(name.clone(), HeaderValue::try_from(value)?);
            }
        }
        let headers = FieldMap::new_immutable(header_map);

        Ok(Self {
            status: value.status,
            headers,
            body: Some(HostIncomingBody::failing(
                "Body stream was interrupted due to a restart".to_string(),
            )), // NOTE: high enough timeout so it does not matter, but not as high to overflow instants
            // Synthetic response: not produced via the connection pool.
            pooled_connection: None,
        })
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
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

#[derive(
    Debug,
    Clone,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
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

#[derive(
    Debug,
    Clone,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
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

#[derive(
    Debug,
    Clone,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
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

impl From<wasmtime_wasi_http::p2::bindings::http::types::ErrorCode> for SerializableHttpErrorCode {
    fn from(value: wasmtime_wasi_http::p2::bindings::http::types::ErrorCode) -> Self {
        (&value).into()
    }
}

impl From<&wasmtime_wasi_http::p2::bindings::http::types::ErrorCode> for SerializableHttpErrorCode {
    fn from(value: &wasmtime_wasi_http::p2::bindings::http::types::ErrorCode) -> Self {
        use wasmtime_wasi_http::p2::bindings::http::types::ErrorCode;

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

impl From<SerializableHttpErrorCode> for wasmtime_wasi_http::p2::bindings::http::types::ErrorCode {
    fn from(value: SerializableHttpErrorCode) -> Self {
        use wasmtime_wasi_http::p2::bindings::http::types::ErrorCode;

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

#[derive(
    Debug,
    Clone,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
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

/// Serializable form of a p3 `http::client::send` request head — the Start
/// payload of the `P3HttpClientSend` oplog pair.
///
/// Records everything needed to identify and replay the outgoing request
/// except the body bytes. The outgoing request body is a `stream<u8>` the guest
/// writes; its bytes are owned by that stream's own durable wrapper (the
/// outgoing-body stream path), so they are intentionally not duplicated here.
/// Whether the consuming `send` already captures those outgoing bytes is a
/// step-3 open question; this payload assumes not and leaves them out.
#[derive(
    Debug,
    Clone,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub struct SerializableP3HttpClientSend {
    pub method: SerializableHttpMethod,
    pub scheme: Option<SerializableP3HttpScheme>,
    pub authority: Option<String>,
    pub path_with_query: Option<String>,
    pub headers: HashMap<String, Vec<Vec<u8>>>,
    pub options: Option<SerializableP3HttpRequestOptions>,
}

/// Serializable form of a p3 request `Scheme`. `http::uri::Scheme` only ever
/// holds `http`/`https` or an arbitrary other scheme string.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub enum SerializableP3HttpScheme {
    Http,
    Https,
    Other(String),
}

/// Serializable form of `wasi:http/types.request-options`.
///
/// All three timeouts are `std::time::Duration`s (unsigned), stored as
/// nanoseconds. The p3 no-clamp rule applies to *signed* instants, so it does
/// not apply here; durations are stored and replayed verbatim.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub struct SerializableP3HttpRequestOptions {
    pub connect_timeout_nanos: Option<u64>,
    pub first_byte_timeout_nanos: Option<u64>,
    pub between_bytes_timeout_nanos: Option<u64>,
}

/// Result of a p3 `http::client::send` — the End payload of the
/// `P3HttpClientSend` oplog pair. This is a result/enum rather than a bare
/// status+headers so that a replayed transport/protocol `ErrorCode` (the gap
/// blocker #2 closes) round-trips back to the guest exactly as it did live.
///
/// Only the response *head* (status + headers) lives here. The body and
/// trailers arrive after the body closes and are replayed by the separate
/// `consume_body` payload pair (step 5), not by this result. Hard traps still
/// escape via `CallHandle::trap`.
#[derive(
    Debug,
    Clone,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub enum SerializableP3HttpClientSendResult {
    Success(SerializableResponseHeaders),
    HttpError(SerializableHttpErrorCode),
}

/// Terminal of a p3 response `consume-body` — the End payload of the
/// `P3HttpClientConsumeBody` oplog pair (step 5).
///
/// The response body *bytes* are recorded separately in the End payload's
/// `contents` field and replayed lazily; this enum captures only how the body
/// stream terminated:
///
/// * `Trailers(None)` — the body closed cleanly with no trailers.
/// * `Trailers(Some(..))` — the body closed cleanly and delivered trailers.
/// * `HttpError(..)` — the body errored before completing. In p3 a body error
///   is surfaced to the guest via the trailers future's `ErrorCode`, not via
///   the body stream, so it round-trips here rather than on the stream.
///
/// `contents` is recorded regardless of this terminal (partial bytes on
/// error/cancel must replay), so an `HttpError` terminal can still carry the
/// bytes that were observed before the failure.
#[derive(
    Debug,
    Clone,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub enum SerializableP3HttpConsumeBodyResult {
    Trailers(Option<HashMap<String, Vec<Vec<u8>>>>),
    HttpError(SerializableHttpErrorCode),
}

/// A subset of AgentMetadata visible for guests (and serializable to oplog)
#[derive(
    Debug,
    Clone,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
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

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
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

// Schema-native A2 impl: a flat enum mirroring the legacy schema
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

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
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

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub enum SerializableP3IpAddress {
    IPv4 { address: [u8; 4] },
    IPv6 { address: [u16; 8] },
}

impl From<p3_socket_types::IpAddress> for SerializableP3IpAddress {
    fn from(value: p3_socket_types::IpAddress) -> Self {
        match value {
            p3_socket_types::IpAddress::Ipv4(address) => SerializableP3IpAddress::IPv4 {
                address: [address.0, address.1, address.2, address.3],
            },
            p3_socket_types::IpAddress::Ipv6(address) => SerializableP3IpAddress::IPv6 {
                address: [
                    address.0, address.1, address.2, address.3, address.4, address.5, address.6,
                    address.7,
                ],
            },
        }
    }
}

impl From<SerializableP3IpAddress> for p3_socket_types::IpAddress {
    fn from(value: SerializableP3IpAddress) -> Self {
        match value {
            SerializableP3IpAddress::IPv4 { address } => {
                p3_socket_types::IpAddress::Ipv4((address[0], address[1], address[2], address[3]))
            }
            SerializableP3IpAddress::IPv6 { address } => p3_socket_types::IpAddress::Ipv6((
                address[0], address[1], address[2], address[3], address[4], address[5], address[6],
                address[7],
            )),
        }
    }
}

impl crate::schema::conversion::IntoSchema for SerializableP3IpAddress {
    fn type_id() -> TypeId {
        TypeId::new("golem_common.model.oplog.payload.SerializableP3IpAddress")
    }

    fn register_in(_b: &mut SchemaBuilder) -> SchemaType {
        SchemaType::string()
    }

    fn to_value(&self) -> SchemaValue {
        let addr = match self {
            SerializableP3IpAddress::IPv4 { address } => IpAddr::V4((*address).into()),
            SerializableP3IpAddress::IPv6 { address } => IpAddr::V6((*address).into()),
        };
        SchemaValue::String(addr.to_string())
    }
}

impl crate::schema::conversion::FromSchema for SerializableP3IpAddress {
    fn from_value(v: &SchemaValue) -> Result<Self, FromSchemaError> {
        match v {
            SchemaValue::String(s) => {
                let ipaddr =
                    IpAddr::from_str(s).map_err(|err| FromSchemaError::custom(err.to_string()))?;
                match ipaddr {
                    IpAddr::V4(addr) => Ok(SerializableP3IpAddress::IPv4 {
                        address: addr.octets(),
                    }),
                    IpAddr::V6(addr) => Ok(SerializableP3IpAddress::IPv6 {
                        address: addr.segments(),
                    }),
                }
            }
            other => Err(FromSchemaError::shape_mismatch(
                "string",
                value_kind(other),
                "SerializableP3IpAddress",
            )),
        }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(transparent)]
pub struct SerializableP3IpAddresses(pub Vec<SerializableP3IpAddress>);

impl From<Vec<p3_socket_types::IpAddress>> for SerializableP3IpAddresses {
    fn from(value: Vec<p3_socket_types::IpAddress>) -> Self {
        SerializableP3IpAddresses(value.into_iter().map(|v| v.into()).collect())
    }
}

impl From<SerializableP3IpAddresses> for Vec<p3_socket_types::IpAddress> {
    fn from(value: SerializableP3IpAddresses) -> Self {
        value.0.into_iter().map(|v| v.into()).collect()
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub struct SerializableP3IpSocketAddress {
    pub address: SerializableP3IpAddress,
    pub port: u16,
    pub flow_info: Option<u32>,
    pub scope_id: Option<u32>,
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

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub struct SerializableP3UdpDatagram {
    pub data: Vec<u8>,
    pub remote_address: SerializableP3IpSocketAddress,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub enum SerializableP3IpNameLookupError {
    AccessDenied,
    InvalidArgument,
    NameUnresolvable,
    TemporaryResolverFailure,
    PermanentResolverFailure,
    Other(Option<String>),
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

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub enum SerializableP3SocketErrorCode {
    AccessDenied,
    NotSupported,
    InvalidArgument,
    OutOfMemory,
    Timeout,
    InvalidState,
    AddressNotBindable,
    AddressInUse,
    RemoteUnreachable,
    ConnectionRefused,
    ConnectionBroken,
    ConnectionReset,
    ConnectionAborted,
    DatagramTooLarge,
    Other(Option<String>),
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

#[derive(
    Debug,
    Clone,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub enum SerializableInvokeResult {
    Failed(String),
    Pending,
    Completed(Result<SchemaValue, SerializableRpcError>),
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub enum SerializableRpcError {
    ProtocolError { details: String },
    Denied { details: String },
    NotFound { details: String },
    RemoteInternalError { details: String },
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub struct SerializableWebsocketCloseInfo {
    pub code: u16,
    pub reason: String,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub enum SerializableWebsocketMessage {
    Text(String),
    Binary(Vec<u8>),
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub enum SerializableWebsocketError {
    ConnectionFailure(String),
    SendFailure(String),
    ReceiveFailure(String),
    ProtocolError(String),
    Closed(Option<SerializableWebsocketCloseInfo>),
    Other(String),
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
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

#[derive(
    Clone,
    Debug,
    Eq,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
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

// Schema-native A2 impl: a flat enum mirroring the legacy schema
// above: the MAC address rendered as a string.
impl crate::schema::conversion::IntoSchema for SerializableMacAddress {
    fn type_id() -> TypeId {
        TypeId::new("golem_common.model.oplog.payload.SerializableMacAddress")
    }
    fn register_in(_b: &mut SchemaBuilder) -> SchemaType {
        SchemaType::string()
    }
    fn to_value(&self) -> SchemaValue {
        SchemaValue::String(self.0.to_string())
    }
}

impl crate::schema::conversion::FromSchema for SerializableMacAddress {
    fn from_value(v: &SchemaValue) -> Result<Self, FromSchemaError> {
        match v {
            SchemaValue::String(s) => {
                let macaddr = MacAddress::from_str(s)
                    .map_err(|err| FromSchemaError::custom(err.to_string()))?;
                Ok(SerializableMacAddress(macaddr))
            }
            other => Err(FromSchemaError::shape_mismatch(
                "string",
                value_kind(other),
                "SerializableMacAddress",
            )),
        }
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

#[derive(
    Clone,
    Debug,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub struct SerializableRdbmsRequest {
    pub pool_key: RdbmsPoolKey,
    pub statement: String,
    pub params: Vec<SerializableDbValue>,
    pub transaction_id: Option<TransactionId>,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub struct SerializableDbValue {
    pub nodes: Vec<SerializableDbValueNode>,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
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

#[derive(
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
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

#[derive(
    Clone,
    Debug,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
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

#[derive(
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
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

#[derive(
    Clone,
    Debug,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
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

#[derive(
    Clone,
    Debug,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub struct SerializableComposite {
    pub name: String,
    pub values: Vec<NodeIndex>,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub struct SerializableDomain {
    pub name: String,
    pub value: NodeIndex,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub struct SerializableRange {
    pub name: String,
    pub value: ValuesRange<NodeIndex>,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
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

#[derive(
    Clone,
    Debug,
    Eq,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
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

#[derive(
    Clone,
    Debug,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
pub struct SerializableCompositeType {
    pub name: String,
    pub attributes: Vec<(String, NodeIndex)>,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
pub struct SerializableDomainType {
    pub name: String,
    pub base_type: NodeIndex,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
pub struct SerializableRangeType {
    pub name: String,
    pub base_type: NodeIndex,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
pub struct SerializableDbColumnType {
    pub nodes: Vec<SerializableDbColumnTypeNode>,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
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

#[derive(
    Clone,
    Debug,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub struct SerializableDbColumn {
    pub ordinal: u64,
    pub name: String,
    pub db_type: SerializableDbColumnType,
    pub db_type_name: String,
}

#[derive(
    Clone,
    Debug,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
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
