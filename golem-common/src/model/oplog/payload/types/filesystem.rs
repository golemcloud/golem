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

use desert_rust::{
    BinaryDeserializer, BinaryInput, BinaryOutput, BinarySerializer, DeserializationContext,
    SerializationContext,
};
use wasmtime_wasi::p2::FsError;
use wasmtime_wasi::p2::bindings::filesystem;
use wasmtime_wasi::p3::bindings::filesystem as p3_filesystem;

use super::{
    FileSystemError, SerializableFsErrorCode, SerializableP3FileSystemError,
    SerializableP3FsErrorCode,
};
use crate::schema::conversion::{FromSchemaError, SchemaBuilder, value_kind};
use crate::schema::metadata::TypeId;
use crate::schema::schema_type::SchemaType;
use crate::schema::schema_value::SchemaValue;

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

impl SerializableP3FileSystemError {
    pub fn from_result(result: Result<p3_filesystem::types::ErrorCode, String>) -> Self {
        match result {
            Ok(error_code) => Self::ErrorCode(SerializableP3FsErrorCode::from(error_code)),
            Err(msg) => SerializableP3FileSystemError::Generic(msg),
        }
    }
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

// Schema-native representation mirroring the legacy schema
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
