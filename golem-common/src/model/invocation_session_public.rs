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

use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Number, Value};
use std::collections::BTreeSet;
use std::fmt::{Display, Formatter};
use uuid::Uuid;

pub const INVOCATION_SESSION_SUBPROTOCOL: &str = "golem.agent-invocation.v1";
pub const INVOCATION_SESSION_VERSION: u8 = 1;
pub const MAX_WEBSOCKET_MESSAGE_SIZE: usize = 32 * 1024 * 1024;
pub const MAX_BINARY_METADATA_SIZE: usize = 16 * 1024;
pub const MAX_PACKED_U8_SIZE: usize = 1024 * 1024;
pub const MAX_LOGICAL_VALUE_SIZE: usize = 16 * 1024 * 1024;
pub const MAX_JSON_DEPTH: usize = 64;
pub const MAX_COLLECTION_SIZE: usize = 100_000;
pub const MAX_STREAM_MAPPINGS: usize = 4096;
pub const MAX_TOKEN_SIZE: usize = 8192;
pub const MAX_IDEMPOTENCY_KEY_SIZE: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicErrorCode {
    UnsupportedSubprotocol,
    AuthenticationFailed,
    Unauthorized,
    NotFound,
    ValidationError,
    UnsupportedValue,
    MalformedMessage,
    UnsupportedVersion,
    ProtocolError,
    InvalidChannel,
    InvalidSequence,
    StreamAlreadyConsumed,
    StreamConflict,
    IdempotencyConflict,
    AttemptConflict,
    StaleSession,
    FutureSession,
    InvalidAttachmentState,
    InputConflict,
    InputGap,
    InvalidCursor,
    TokenInvalid,
    ResourceExhausted,
    ProducerError,
    InvocationFailed,
    InternalError,
}

impl PublicErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedSubprotocol => "unsupported-subprotocol",
            Self::AuthenticationFailed => "authentication-failed",
            Self::Unauthorized => "unauthorized",
            Self::NotFound => "not-found",
            Self::ValidationError => "validation-error",
            Self::UnsupportedValue => "unsupported-value",
            Self::MalformedMessage => "malformed-message",
            Self::UnsupportedVersion => "unsupported-version",
            Self::ProtocolError => "protocol-error",
            Self::InvalidChannel => "invalid-channel",
            Self::InvalidSequence => "invalid-sequence",
            Self::StreamAlreadyConsumed => "stream-already-consumed",
            Self::StreamConflict => "stream-conflict",
            Self::IdempotencyConflict => "idempotency-conflict",
            Self::AttemptConflict => "attempt-conflict",
            Self::StaleSession => "stale-session",
            Self::FutureSession => "future-session",
            Self::InvalidAttachmentState => "invalid-attachment-state",
            Self::InputConflict => "input-conflict",
            Self::InputGap => "input-gap",
            Self::InvalidCursor => "invalid-cursor",
            Self::TokenInvalid => "token-invalid",
            Self::ResourceExhausted => "resource-exhausted",
            Self::ProducerError => "producer-error",
            Self::InvocationFailed => "invocation-failed",
            Self::InternalError => "internal-error",
        }
    }
}

impl Serialize for PublicErrorCode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for PublicErrorCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        ALL_ERROR_CODES
            .iter()
            .copied()
            .find(|code| code.as_str() == value)
            .ok_or_else(|| serde::de::Error::custom("unknown public error code"))
    }
}

const ALL_ERROR_CODES: &[PublicErrorCode] = &[
    PublicErrorCode::UnsupportedSubprotocol,
    PublicErrorCode::AuthenticationFailed,
    PublicErrorCode::Unauthorized,
    PublicErrorCode::NotFound,
    PublicErrorCode::ValidationError,
    PublicErrorCode::UnsupportedValue,
    PublicErrorCode::MalformedMessage,
    PublicErrorCode::UnsupportedVersion,
    PublicErrorCode::ProtocolError,
    PublicErrorCode::InvalidChannel,
    PublicErrorCode::InvalidSequence,
    PublicErrorCode::StreamAlreadyConsumed,
    PublicErrorCode::StreamConflict,
    PublicErrorCode::IdempotencyConflict,
    PublicErrorCode::AttemptConflict,
    PublicErrorCode::StaleSession,
    PublicErrorCode::FutureSession,
    PublicErrorCode::InvalidAttachmentState,
    PublicErrorCode::InputConflict,
    PublicErrorCode::InputGap,
    PublicErrorCode::InvalidCursor,
    PublicErrorCode::TokenInvalid,
    PublicErrorCode::ResourceExhausted,
    PublicErrorCode::ProducerError,
    PublicErrorCode::InvocationFailed,
    PublicErrorCode::InternalError,
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicProtocolError {
    pub code: PublicErrorCode,
    pub message: String,
}

impl PublicProtocolError {
    pub fn new(code: PublicErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl Display for PublicProtocolError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for PublicProtocolError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DecimalU64(pub u64);

impl Serialize for DecimalU64 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> Deserialize<'de> for DecimalU64 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if value.is_empty()
            || (value.len() > 1 && value.starts_with('0'))
            || !value.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(serde::de::Error::custom(
                "expected a canonical unsigned decimal string",
            ));
        }
        value
            .parse()
            .map(Self)
            .map_err(|_| serde::de::Error::custom("unsigned decimal string exceeds u64"))
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct InvocationSelector {
    pub agent_type: String,
    pub application: String,
    pub constructor_parameters: Value,
    pub environment: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phantom_id: Option<Uuid>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PublicConfigEntry {
    pub path: Vec<String>,
    pub value: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PublicStreamDirection {
    Input,
    Output,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PublicInputHighWater {
    pub sequence: DecimalU64,
    pub terminal: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PublicStreamMapping {
    pub channel: u32,
    pub direction: PublicStreamDirection,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_high_water: Option<PublicInputHighWater>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provisional_ref: Option<Uuid>,
    pub stream_token: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PublicClientCancelReason {
    Cancelled,
    ConsumerDrop,
    SourceUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PublicServerCancelReason {
    Cancelled,
    ConsumerDrop,
    TransportDetached,
    SourceUnavailable,
    ProducerDeleted,
    InvocationFailed,
    ProtocolError,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PublicAttachmentRevokedReason {
    Replaced,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PublicResumeOperation {
    Resume,
    Takeover,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum PublicClientMessage {
    #[serde(rename = "invocationStart")]
    InvocationStart {
        #[serde(rename = "attemptId")]
        attempt_id: Uuid,
        config: Vec<PublicConfigEntry>,
        #[serde(rename = "idempotencyKey")]
        idempotency_key: String,
        #[serde(rename = "methodParameters")]
        method_parameters: Value,
        selector: InvocationSelector,
        version: u8,
    },
    #[serde(rename = "resumeAttach")]
    ResumeAttach {
        #[serde(rename = "attemptId")]
        attempt_id: Uuid,
        operation: PublicResumeOperation,
        #[serde(rename = "outputCursors")]
        output_cursors: Vec<String>,
        #[serde(rename = "sessionToken")]
        session_token: String,
        version: u8,
    },
    #[serde(rename = "inputStreamItem")]
    InputStreamItem {
        channel: u32,
        sequence: DecimalU64,
        value: Value,
        version: u8,
    },
    #[serde(rename = "inputStreamEnd")]
    InputStreamEnd {
        channel: u32,
        sequence: DecimalU64,
        version: u8,
    },
    #[serde(rename = "streamCancel")]
    StreamCancel {
        channel: u32,
        reason: PublicClientCancelReason,
        version: u8,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum PublicInvocationResult {
    None,
    Value { value: Value },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum PublicOutputStreamOutcome {
    Ok,
    Error {
        code: PublicErrorCode,
        message: String,
    },
    Cancelled {
        reason: PublicServerCancelReason,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum PublicInvocationOutcome {
    Success,
    Failure {
        code: PublicErrorCode,
        message: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum PublicServerMessage {
    #[serde(rename = "invocationAccepted")]
    InvocationAccepted {
        #[serde(rename = "attemptId")]
        attempt_id: Uuid,
        #[serde(rename = "idempotencyKey")]
        idempotency_key: String,
        mappings: Vec<PublicStreamMapping>,
        #[serde(rename = "sessionToken")]
        session_token: String,
        version: u8,
    },
    #[serde(rename = "invocationRejected")]
    InvocationRejected {
        #[serde(default, rename = "attemptId", skip_serializing_if = "Option::is_none")]
        attempt_id: Option<Uuid>,
        code: PublicErrorCode,
        message: String,
        retryable: bool,
        version: u8,
    },
    #[serde(rename = "invocationResult")]
    InvocationResult {
        mappings: Vec<PublicStreamMapping>,
        result: PublicInvocationResult,
        version: u8,
    },
    #[serde(rename = "outputStreamItem")]
    OutputStreamItem {
        channel: u32,
        #[serde(rename = "cursorToken")]
        cursor_token: String,
        mappings: Vec<PublicStreamMapping>,
        sequence: DecimalU64,
        value: Value,
        version: u8,
    },
    #[serde(rename = "outputStreamEnd")]
    OutputStreamEnd {
        channel: u32,
        #[serde(
            default,
            rename = "cursorToken",
            skip_serializing_if = "Option::is_none"
        )]
        cursor_token: Option<String>,
        outcome: PublicOutputStreamOutcome,
        sequence: DecimalU64,
        version: u8,
    },
    #[serde(rename = "inputStreamAck")]
    InputStreamAck {
        channel: u32,
        #[serde(rename = "highestContiguousSequence")]
        highest_contiguous_sequence: DecimalU64,
        mappings: Vec<PublicStreamMapping>,
        terminal: bool,
        version: u8,
    },
    #[serde(rename = "streamCancel")]
    StreamCancel {
        channel: u32,
        reason: PublicServerCancelReason,
        version: u8,
    },
    #[serde(rename = "attachmentRevoked")]
    AttachmentRevoked {
        reason: PublicAttachmentRevokedReason,
        version: u8,
    },
    #[serde(rename = "invocationFinished")]
    InvocationFinished {
        outcome: PublicInvocationOutcome,
        version: u8,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinaryMessageKind {
    #[serde(rename = "input-u8")]
    InputU8,
    #[serde(rename = "output-u8")]
    OutputU8,
    #[serde(rename = "input-binary")]
    InputBinary,
    #[serde(rename = "output-binary")]
    OutputBinary,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BinaryMessageMetadata {
    pub channel: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_token: Option<String>,
    pub item_count: DecimalU64,
    pub kind: BinaryMessageKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    pub sequence: DecimalU64,
    pub version: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BinaryMessage {
    pub metadata: BinaryMessageMetadata,
    pub payload: Vec<u8>,
}

pub fn decode_client_text(bytes: &[u8]) -> Result<PublicClientMessage, PublicProtocolError> {
    validate_message_size(bytes.len())?;
    let value = parse_strict_json(bytes)?;
    check_version(&value)?;
    prevalidate_decimal_fields(&value)?;
    prevalidate_uuid_fields(&value)?;
    let message: PublicClientMessage = serde_json::from_value(value).map_err(|_| {
        PublicProtocolError::new(PublicErrorCode::MalformedMessage, "invalid client message")
    })?;
    validate_client_message(&message)?;
    Ok(message)
}

pub fn decode_server_text(bytes: &[u8]) -> Result<PublicServerMessage, PublicProtocolError> {
    validate_message_size(bytes.len())?;
    let value = parse_strict_json(bytes)?;
    check_version(&value)?;
    prevalidate_decimal_fields(&value)?;
    prevalidate_uuid_fields(&value)?;
    let message: PublicServerMessage = serde_json::from_value(value).map_err(|_| {
        PublicProtocolError::new(PublicErrorCode::MalformedMessage, "invalid server message")
    })?;
    validate_server_message(&message)?;
    Ok(message)
}

pub fn encode_text(message: &impl Serialize) -> Result<String, PublicProtocolError> {
    let value = serde_json::to_value(message).map_err(|_| {
        PublicProtocolError::new(
            PublicErrorCode::InternalError,
            "failed to encode protocol message",
        )
    })?;
    let encoded = encode_json_value(&value)?;
    validate_message_size(encoded.len())?;
    Ok(encoded)
}

pub fn encode_json_value(value: &Value) -> Result<String, PublicProtocolError> {
    let mut output = String::new();
    write_json_value(value, &mut output)?;
    Ok(output)
}

fn write_json_value(value: &Value, output: &mut String) -> Result<(), PublicProtocolError> {
    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(value) => output.push_str(if *value { "true" } else { "false" }),
        Value::Number(number) => {
            if number.is_f64()
                && number
                    .as_f64()
                    .is_some_and(|value| value == 0.0 && value.is_sign_negative())
            {
                output.push_str("-0");
            } else {
                output.push_str(&number.to_string());
            }
        }
        Value::String(value) => output.push_str(&serde_json::to_string(value).map_err(|_| {
            PublicProtocolError::new(
                PublicErrorCode::InternalError,
                "failed to encode JSON string",
            )
        })?),
        Value::Array(values) => {
            output.push('[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                write_json_value(value, output)?;
            }
            output.push(']');
        }
        Value::Object(values) => {
            output.push('{');
            let mut entries = values.iter().collect::<Vec<_>>();
            entries.sort_unstable_by_key(|(key, _)| *key);
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                output.push_str(&serde_json::to_string(key).map_err(|_| {
                    PublicProtocolError::new(
                        PublicErrorCode::InternalError,
                        "failed to encode JSON object member",
                    )
                })?);
                output.push(':');
                write_json_value(value, output)?;
            }
            output.push('}');
        }
    }
    Ok(())
}

pub fn decode_binary_message(bytes: &[u8]) -> Result<BinaryMessage, PublicProtocolError> {
    validate_message_size(bytes.len())?;
    if bytes.len() < 4 {
        return Err(PublicProtocolError::new(
            PublicErrorCode::MalformedMessage,
            "binary message has no complete metadata length",
        ));
    }
    let metadata_len = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
    if metadata_len > MAX_BINARY_METADATA_SIZE {
        return Err(PublicProtocolError::new(
            PublicErrorCode::ResourceExhausted,
            "binary metadata exceeds 16 KiB",
        ));
    }
    let metadata_end = 4usize.checked_add(metadata_len).ok_or_else(|| {
        PublicProtocolError::new(
            PublicErrorCode::MalformedMessage,
            "binary metadata length overflow",
        )
    })?;
    if metadata_end > bytes.len() {
        return Err(PublicProtocolError::new(
            PublicErrorCode::MalformedMessage,
            "binary metadata length exceeds the message",
        ));
    }
    let value = parse_strict_json(&bytes[4..metadata_end])?;
    check_version(&value)?;
    prevalidate_decimal_fields(&value)?;
    let metadata: BinaryMessageMetadata = serde_json::from_value(value).map_err(|_| {
        PublicProtocolError::new(PublicErrorCode::MalformedMessage, "invalid binary metadata")
    })?;
    let payload = bytes[metadata_end..].to_vec();
    validate_binary_metadata(&metadata, payload.len())?;
    Ok(BinaryMessage { metadata, payload })
}

pub fn encode_binary_message(message: &BinaryMessage) -> Result<Vec<u8>, PublicProtocolError> {
    validate_binary_metadata(&message.metadata, message.payload.len())?;
    let metadata = encode_text(&message.metadata)?.into_bytes();
    if metadata.len() > MAX_BINARY_METADATA_SIZE {
        return Err(PublicProtocolError::new(
            PublicErrorCode::ResourceExhausted,
            "binary metadata exceeds 16 KiB",
        ));
    }
    let mut bytes = Vec::with_capacity(4 + metadata.len() + message.payload.len());
    bytes.extend_from_slice(&(metadata.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&metadata);
    bytes.extend_from_slice(&message.payload);
    validate_message_size(bytes.len())?;
    Ok(bytes)
}

fn validate_message_size(size: usize) -> Result<(), PublicProtocolError> {
    if size > MAX_WEBSOCKET_MESSAGE_SIZE {
        Err(PublicProtocolError::new(
            PublicErrorCode::ResourceExhausted,
            "WebSocket application message exceeds 32 MiB",
        ))
    } else {
        Ok(())
    }
}

pub fn parse_strict_json(bytes: &[u8]) -> Result<Value, PublicProtocolError> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = StrictJsonValue::deserialize(&mut deserializer)
        .map_err(|_| PublicProtocolError::new(PublicErrorCode::MalformedMessage, "invalid JSON"))?
        .0;
    deserializer.end().map_err(|_| {
        PublicProtocolError::new(
            PublicErrorCode::MalformedMessage,
            "trailing data after JSON value",
        )
    })?;
    validate_json_limits(&value, 0)?;
    Ok(value)
}

fn check_version(value: &Value) -> Result<(), PublicProtocolError> {
    match value.as_object().and_then(|object| object.get("version")) {
        Some(Value::Number(number)) if number.as_u64() == Some(1) => Ok(()),
        Some(Value::Number(_)) => Err(PublicProtocolError::new(
            PublicErrorCode::UnsupportedVersion,
            "unsupported protocol version",
        )),
        _ => Err(PublicProtocolError::new(
            PublicErrorCode::MalformedMessage,
            "protocol version must be the number 1",
        )),
    }
}

fn prevalidate_decimal_fields(value: &Value) -> Result<(), PublicProtocolError> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    for name in ["sequence", "itemCount", "highestContiguousSequence"] {
        let Some(value) = object.get(name) else {
            continue;
        };
        let Some(value) = value.as_str() else {
            return Err(PublicProtocolError::new(
                PublicErrorCode::MalformedMessage,
                format!("{name} must be a canonical unsigned decimal string"),
            ));
        };
        if value.is_empty()
            || (value.len() > 1 && value.starts_with('0'))
            || !value.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(PublicProtocolError::new(
                PublicErrorCode::MalformedMessage,
                format!("{name} must be a canonical unsigned decimal string"),
            ));
        }
        if value.parse::<u64>().is_err() {
            return Err(PublicProtocolError::new(
                PublicErrorCode::InvalidSequence,
                format!("{name} exceeds u64"),
            ));
        }
    }
    Ok(())
}

fn prevalidate_uuid_fields(value: &Value) -> Result<(), PublicProtocolError> {
    let object = value.as_object().ok_or_else(|| {
        PublicProtocolError::new(
            PublicErrorCode::MalformedMessage,
            "protocol message must be an object",
        )
    })?;
    if let Some(attempt_id) = object.get("attemptId") {
        validate_canonical_uuid(attempt_id, true, "attempt ID")?;
    }
    if let Some(selector) = object.get("selector").and_then(Value::as_object)
        && let Some(phantom_id) = selector.get("phantomId")
    {
        validate_canonical_uuid(phantom_id, false, "phantom ID")?;
    }
    if let Some(mappings) = object.get("mappings").and_then(Value::as_array) {
        for mapping in mappings {
            if let Some(provisional_ref) = mapping
                .as_object()
                .and_then(|mapping| mapping.get("provisionalRef"))
            {
                validate_canonical_uuid(provisional_ref, true, "provisional stream reference")?;
            }
        }
    }
    Ok(())
}

fn validate_canonical_uuid(
    value: &Value,
    require_v4: bool,
    name: &'static str,
) -> Result<(), PublicProtocolError> {
    let encoded = value.as_str().ok_or_else(|| {
        PublicProtocolError::new(
            PublicErrorCode::ValidationError,
            format!("{name} must be a canonical UUID"),
        )
    })?;
    let parsed = encoded.parse::<Uuid>().map_err(|_| {
        PublicProtocolError::new(
            PublicErrorCode::ValidationError,
            format!("{name} must be a canonical UUID"),
        )
    })?;
    if parsed.hyphenated().to_string() != encoded
        || parsed.get_variant() != uuid::Variant::RFC4122
        || (require_v4 && parsed.get_version_num() != 4)
    {
        return Err(PublicProtocolError::new(
            PublicErrorCode::ValidationError,
            format!(
                "{name} must be a canonical UUID{}",
                if require_v4 { "v4" } else { "" }
            ),
        ));
    }
    Ok(())
}

fn validate_client_message(message: &PublicClientMessage) -> Result<(), PublicProtocolError> {
    match message {
        PublicClientMessage::InvocationStart {
            attempt_id,
            config,
            idempotency_key,
            selector,
            ..
        } => {
            validate_uuid_v4(*attempt_id, "attempt ID")?;
            if idempotency_key.is_empty() || idempotency_key.len() > MAX_IDEMPOTENCY_KEY_SIZE {
                return Err(PublicProtocolError::new(
                    PublicErrorCode::ValidationError,
                    "invalid idempotency key",
                ));
            }
            if selector.application.is_empty()
                || selector.environment.is_empty()
                || selector.agent_type.is_empty()
                || selector.method.is_empty()
            {
                return Err(PublicProtocolError::new(
                    PublicErrorCode::ValidationError,
                    "selector names must not be empty",
                ));
            }
            for entry in config {
                if entry.path.is_empty() || entry.path.iter().any(String::is_empty) {
                    return Err(PublicProtocolError::new(
                        PublicErrorCode::ValidationError,
                        "configuration paths must not be empty",
                    ));
                }
                if contains_public_stream_reference(&entry.value) {
                    return Err(PublicProtocolError::new(
                        PublicErrorCode::UnsupportedValue,
                        "configuration values must not contain stream references",
                    ));
                }
            }
        }
        PublicClientMessage::ResumeAttach {
            attempt_id,
            output_cursors,
            session_token,
            ..
        } => {
            validate_uuid_v4(*attempt_id, "attempt ID")?;
            validate_token_text(session_token)?;
            let mut distinct = BTreeSet::new();
            for cursor in output_cursors {
                validate_token_text(cursor)?;
                if !distinct.insert(cursor) {
                    return Err(PublicProtocolError::new(
                        PublicErrorCode::InvalidCursor,
                        "output cursor tokens must be distinct",
                    ));
                }
            }
        }
        PublicClientMessage::InputStreamItem { channel, .. }
        | PublicClientMessage::InputStreamEnd { channel, .. }
        | PublicClientMessage::StreamCancel { channel, .. } => validate_channel(*channel)?,
    }
    Ok(())
}

fn contains_public_stream_reference(value: &Value) -> bool {
    match value {
        Value::Array(values) => values.iter().any(contains_public_stream_reference),
        Value::Object(values) => {
            values.get("$stream").is_some_and(|value| {
                value.as_object().is_some_and(|stream| {
                    stream.contains_key("provisionalRef") || stream.contains_key("streamToken")
                })
            }) || values.values().any(contains_public_stream_reference)
        }
        _ => false,
    }
}

fn validate_server_message(message: &PublicServerMessage) -> Result<(), PublicProtocolError> {
    match message {
        PublicServerMessage::InvocationAccepted {
            attempt_id,
            mappings,
            session_token,
            ..
        } => {
            validate_uuid_v4(*attempt_id, "attempt ID")?;
            validate_token_text(session_token)?;
            validate_mappings(mappings)?;
        }
        PublicServerMessage::InvocationRejected { attempt_id, .. } => {
            if let Some(attempt_id) = attempt_id {
                validate_uuid_v4(*attempt_id, "attempt ID")?;
            }
        }
        PublicServerMessage::InvocationResult { mappings, .. } => validate_mappings(mappings)?,
        PublicServerMessage::InputStreamAck {
            channel, mappings, ..
        } => {
            validate_channel(*channel)?;
            validate_mappings(mappings)?;
        }
        PublicServerMessage::OutputStreamItem {
            channel,
            cursor_token,
            mappings,
            ..
        } => {
            validate_channel(*channel)?;
            validate_token_text(cursor_token)?;
            validate_mappings(mappings)?;
        }
        PublicServerMessage::OutputStreamEnd {
            channel,
            cursor_token,
            ..
        } => {
            validate_channel(*channel)?;
            if let Some(cursor) = cursor_token {
                validate_token_text(cursor)?;
            }
        }
        PublicServerMessage::StreamCancel { channel, .. } => validate_channel(*channel)?,
        PublicServerMessage::AttachmentRevoked { .. }
        | PublicServerMessage::InvocationFinished { .. } => {}
    }
    Ok(())
}

fn validate_mappings(mappings: &[PublicStreamMapping]) -> Result<(), PublicProtocolError> {
    if mappings.len() > MAX_STREAM_MAPPINGS {
        return Err(PublicProtocolError::new(
            PublicErrorCode::ResourceExhausted,
            "too many stream mappings",
        ));
    }
    let mut channels = BTreeSet::new();
    let mut tokens = BTreeSet::new();
    let mut provisional_refs = BTreeSet::new();
    for mapping in mappings {
        validate_channel(mapping.channel)?;
        validate_token_text(&mapping.stream_token)?;
        if !channels.insert(mapping.channel) || !tokens.insert(&mapping.stream_token) {
            return Err(PublicProtocolError::new(
                PublicErrorCode::StreamConflict,
                "stream mappings must have distinct channels and tokens",
            ));
        }
        if let Some(reference) = mapping.provisional_ref {
            validate_uuid_v4(reference, "provisional stream reference")?;
            if !provisional_refs.insert(reference) {
                return Err(PublicProtocolError::new(
                    PublicErrorCode::StreamConflict,
                    "stream mappings must have distinct provisional references",
                ));
            }
        }
        match mapping.direction {
            PublicStreamDirection::Input if mapping.input_high_water.is_none() => {
                return Err(PublicProtocolError::new(
                    PublicErrorCode::StreamConflict,
                    "input stream mapping has no high-water mark",
                ));
            }
            PublicStreamDirection::Output if mapping.input_high_water.is_some() => {
                return Err(PublicProtocolError::new(
                    PublicErrorCode::StreamConflict,
                    "output stream mapping has an input high-water mark",
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_uuid_v4(value: Uuid, name: &'static str) -> Result<(), PublicProtocolError> {
    if value.get_variant() != uuid::Variant::RFC4122 || value.get_version_num() != 4 {
        return Err(PublicProtocolError::new(
            PublicErrorCode::ValidationError,
            format!("{name} must be UUIDv4"),
        ));
    }
    Ok(())
}

fn validate_binary_metadata(
    metadata: &BinaryMessageMetadata,
    payload_len: usize,
) -> Result<(), PublicProtocolError> {
    if metadata.version != INVOCATION_SESSION_VERSION {
        return Err(PublicProtocolError::new(
            PublicErrorCode::UnsupportedVersion,
            "unsupported binary protocol version",
        ));
    }
    validate_channel(metadata.channel)?;
    metadata
        .sequence
        .0
        .checked_add(metadata.item_count.0)
        .ok_or_else(|| {
            PublicProtocolError::new(
                PublicErrorCode::InvalidSequence,
                "binary sequence range overflows u64",
            )
        })?;
    match metadata.kind {
        BinaryMessageKind::InputU8 | BinaryMessageKind::OutputU8 => {
            if payload_len == 0 || payload_len > MAX_PACKED_U8_SIZE {
                return Err(PublicProtocolError::new(
                    if payload_len > MAX_PACKED_U8_SIZE {
                        PublicErrorCode::ResourceExhausted
                    } else {
                        PublicErrorCode::MalformedMessage
                    },
                    "packed u8 payload must contain 1 through 1 MiB bytes",
                ));
            }
            if metadata.item_count.0 != payload_len as u64 {
                return Err(PublicProtocolError::new(
                    PublicErrorCode::InvalidSequence,
                    "packed u8 item count differs from payload length",
                ));
            }
            if metadata.mime_type.is_some() {
                return Err(PublicProtocolError::new(
                    PublicErrorCode::MalformedMessage,
                    "packed u8 metadata must not contain a MIME type",
                ));
            }
        }
        BinaryMessageKind::InputBinary | BinaryMessageKind::OutputBinary => {
            if payload_len > MAX_LOGICAL_VALUE_SIZE {
                return Err(PublicProtocolError::new(
                    PublicErrorCode::ResourceExhausted,
                    "binary item exceeds 16 MiB",
                ));
            }
            if metadata.item_count.0 != 1 {
                return Err(PublicProtocolError::new(
                    PublicErrorCode::InvalidSequence,
                    "binary item count must be one",
                ));
            }
            if let Some(mime_type) = &metadata.mime_type
                && !valid_mime_type(mime_type)
            {
                return Err(PublicProtocolError::new(
                    PublicErrorCode::ValidationError,
                    "invalid binary MIME type",
                ));
            }
        }
    }
    match metadata.kind {
        BinaryMessageKind::InputU8 | BinaryMessageKind::InputBinary
            if metadata.cursor_token.is_some() =>
        {
            Err(PublicProtocolError::new(
                PublicErrorCode::MalformedMessage,
                "input binary metadata must not contain a cursor",
            ))
        }
        BinaryMessageKind::OutputU8 | BinaryMessageKind::OutputBinary => {
            let cursor = metadata.cursor_token.as_ref().ok_or_else(|| {
                PublicProtocolError::new(
                    PublicErrorCode::MalformedMessage,
                    "output binary metadata requires a cursor",
                )
            })?;
            validate_token_text(cursor)
        }
        _ => Ok(()),
    }
}

fn validate_channel(channel: u32) -> Result<(), PublicProtocolError> {
    if channel == 0 {
        Err(PublicProtocolError::new(
            PublicErrorCode::InvalidChannel,
            "channel zero is reserved",
        ))
    } else {
        Ok(())
    }
}

fn validate_token_text(token: &str) -> Result<(), PublicProtocolError> {
    if token.is_empty() || token.len() > MAX_TOKEN_SIZE {
        Err(PublicProtocolError::new(
            if token.len() > MAX_TOKEN_SIZE {
                PublicErrorCode::ResourceExhausted
            } else {
                PublicErrorCode::TokenInvalid
            },
            "invalid opaque token length",
        ))
    } else {
        Ok(())
    }
}

fn valid_mime_type(value: &str) -> bool {
    let Some((kind, subtype)) = value.split_once('/') else {
        return false;
    };
    !kind.is_empty()
        && !subtype.is_empty()
        && kind.bytes().all(valid_mime_byte)
        && subtype.bytes().all(valid_mime_byte)
}

fn valid_mime_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-'
        )
}

fn validate_json_limits(value: &Value, depth: usize) -> Result<(), PublicProtocolError> {
    if depth > MAX_JSON_DEPTH {
        return Err(PublicProtocolError::new(
            PublicErrorCode::ResourceExhausted,
            "JSON nesting exceeds 64 levels",
        ));
    }
    match value {
        Value::Array(values) => {
            if values.len() > MAX_COLLECTION_SIZE {
                return Err(PublicProtocolError::new(
                    PublicErrorCode::ResourceExhausted,
                    "JSON collection exceeds 100000 elements",
                ));
            }
            for value in values {
                validate_json_limits(value, depth + 1)?;
            }
        }
        Value::Object(values) => {
            if values.len() > MAX_COLLECTION_SIZE {
                return Err(PublicProtocolError::new(
                    PublicErrorCode::ResourceExhausted,
                    "JSON object exceeds 100000 members",
                ));
            }
            for value in values.values() {
                validate_json_limits(value, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

struct StrictJsonValue(Value);

impl<'de> Deserialize<'de> for StrictJsonValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictJsonVisitor)
    }
}

struct StrictJsonVisitor;

impl<'de> Visitor<'de> for StrictJsonVisitor {
    type Value = StrictJsonValue;

    fn expecting(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON value without duplicate object members")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Number(Number::from(value))))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Number(Number::from(value))))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Number::from_f64(value)
            .map(Value::Number)
            .map(StrictJsonValue)
            .ok_or_else(|| E::custom("non-finite JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::String(value.to_string())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(StrictJsonValue(Value::Null))
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: Deserializer<'de>,
    {
        StrictJsonValue::deserialize(deserializer)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<StrictJsonValue>()? {
            values.push(value.0);
        }
        Ok(StrictJsonValue(Value::Array(values)))
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        while let Some(key) = object.next_key::<String>()? {
            let value = object.next_value::<StrictJsonValue>()?;
            if values.insert(key, value.0).is_some() {
                return Err(serde::de::Error::custom("duplicate JSON object member"));
            }
        }
        Ok(StrictJsonValue(Value::Object(values)))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BinaryMessageKind, MAX_WEBSOCKET_MESSAGE_SIZE, PublicClientMessage, PublicErrorCode,
        PublicServerMessage, decode_binary_message, decode_client_text, decode_server_text,
        encode_text, validate_message_size,
    };
    use serde::Deserialize;
    use test_r::test;

    #[derive(Deserialize)]
    struct JsonFixture {
        vectors: Vec<JsonVector>,
    }

    #[derive(Deserialize)]
    struct JsonVector {
        canonical: String,
        direction: String,
    }

    #[test]
    fn canonical_json_fixtures_round_trip() {
        let fixture: JsonFixture = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../golem-client/tests/fixtures/stream-session-v1/json-messages.json"
        )))
        .unwrap();
        for vector in fixture.vectors {
            let encoded = match vector.direction.as_str() {
                "client" => encode_text(&decode_client_text(vector.canonical.as_bytes()).unwrap()),
                "server" => encode_text(&decode_server_text(vector.canonical.as_bytes()).unwrap()),
                other => panic!("unexpected fixture direction {other}"),
            }
            .unwrap();
            assert_eq!(encoded, vector.canonical);
        }
    }

    #[test]
    fn strict_json_rejects_duplicate_and_unknown_fields() {
        let duplicate = br#"{"channel":1,"sequence":"0","type":"inputStreamEnd","type":"streamCancel","version":1}"#;
        assert_eq!(
            decode_client_text(duplicate).unwrap_err().code,
            PublicErrorCode::MalformedMessage
        );
        let unknown =
            br#"{"channel":1,"extra":true,"sequence":"0","type":"inputStreamEnd","version":1}"#;
        assert_eq!(
            decode_client_text(unknown).unwrap_err().code,
            PublicErrorCode::MalformedMessage
        );
        let invalid_revocation =
            br#"{"reason":"attachment revoked","type":"attachmentRevoked","version":1}"#;
        assert_eq!(
            decode_server_text(invalid_revocation).unwrap_err().code,
            PublicErrorCode::MalformedMessage
        );
    }

    #[test]
    fn unsupported_version_is_distinct() {
        let bytes = br#"{"channel":1,"sequence":"0","type":"inputStreamEnd","version":2}"#;
        assert_eq!(
            decode_client_text(bytes).unwrap_err().code,
            PublicErrorCode::UnsupportedVersion
        );
    }

    #[test]
    fn binary_metadata_is_strict_and_validates_lane_rules() {
        let metadata =
            br#"{"channel":1,"itemCount":"2","kind":"input-u8","sequence":"0","version":1}"#;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&(metadata.len() as u32).to_be_bytes());
        bytes.extend_from_slice(metadata);
        bytes.extend_from_slice(&[1, 2]);
        let decoded = decode_binary_message(&bytes).unwrap();
        assert_eq!(decoded.metadata.kind, BinaryMessageKind::InputU8);

        bytes.pop();
        assert_eq!(
            decode_binary_message(&bytes).unwrap_err().code,
            PublicErrorCode::InvalidSequence
        );
    }

    #[test]
    fn public_message_enums_are_directional() {
        let client = br#"{"channel":1,"sequence":"0","type":"inputStreamEnd","version":1}"#;
        assert!(matches!(
            decode_client_text(client).unwrap(),
            PublicClientMessage::InputStreamEnd { .. }
        ));
        assert!(decode_server_text(client).is_err());

        let server = br#"{"outcome":{"kind":"success"},"type":"invocationFinished","version":1}"#;
        assert!(matches!(
            decode_server_text(server).unwrap(),
            PublicServerMessage::InvocationFinished { .. }
        ));
        assert!(decode_client_text(server).is_err());
    }

    #[test]
    fn server_input_ack_rejects_reserved_zero_channel() {
        let input = br#"{"channel":0,"highestContiguousSequence":"0","mappings":[],"terminal":false,"type":"inputStreamAck","version":1}"#;
        let error = decode_server_text(input).expect_err("channel zero must remain reserved");
        assert_eq!(error.code, PublicErrorCode::InvalidChannel);
    }

    #[test]
    fn client_attempt_ids_must_be_uuid_v4() {
        let input = br#"{"attemptId":"00000000-0000-0000-0000-000000000000","config":[],"idempotencyKey":"key","methodParameters":{},"selector":{"agentType":"agent","application":"app","constructorParameters":{},"environment":"env","method":"run"},"type":"invocationStart","version":1}"#;

        assert!(
            decode_client_text(input).is_err(),
            "the public v1 contract requires attempt IDs to be UUIDv4 values"
        );
    }

    #[test]
    fn client_attempt_ids_must_use_the_rfc4122_variant() {
        let input = br#"{"attemptId":"00000000-0000-4000-0000-000000000000","config":[],"idempotencyKey":"key","methodParameters":{},"selector":{"agentType":"agent","application":"app","constructorParameters":{},"environment":"env","method":"run"},"type":"invocationStart","version":1}"#;

        assert!(
            decode_client_text(input).is_err(),
            "UUIDv4 requires the RFC 4122 variant as well as the version-4 nibble"
        );
    }

    #[test]
    fn websocket_application_message_limit_is_inclusive() {
        assert!(validate_message_size(MAX_WEBSOCKET_MESSAGE_SIZE).is_ok());
        assert_eq!(
            validate_message_size(MAX_WEBSOCKET_MESSAGE_SIZE + 1)
                .unwrap_err()
                .code,
            PublicErrorCode::ResourceExhausted
        );
    }
}
