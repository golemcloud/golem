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

use serde::de::{DeserializeOwned, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Formatter};
use uuid::Uuid;

use crate::model::{AgentFingerprint, AgentId, InvocationId};
use crate::schema::SchemaGraph;

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

#[cfg(feature = "full")]
pub fn new_durable_stream_session_id() -> String {
    crate::model::IdempotencyKey::fresh().value
}

/// Validates an invocation idempotency key used as a Durable Streams URL segment.
pub fn validate_durable_stream_session_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(
            "Durable Streams session id must contain 1–128 ASCII letters, digits, '.', '_' or '-'"
                .into(),
        );
    }
    Ok(())
}

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
    /// The agent's previous invocation failed, so the agent accepts no invocation until an
    /// operator reverts or resumes it.
    PreviousInvocationFailed,
    /// The executor that answered does not own this agent's shard right now: the assignment is
    /// moving, or one of its writes was fenced by the shard's new owner. Nothing is wrong with the
    /// request, and a client that retries reaches the new owner - which is why this is not
    /// `InternalError`.
    RoutingMiss,
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
            Self::PreviousInvocationFailed => "previous-invocation-failed",
            Self::RoutingMiss => "routing-miss",
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
    PublicErrorCode::PreviousInvocationFailed,
    PublicErrorCode::RoutingMiss,
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
    pub target: PublicAgentTarget,
    pub environment: String,
    pub method: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PublicAgentTarget {
    Key {
        constructor_parameters: Value,
    },
    Id {
        agent_id: AgentId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_fingerprint: Option<AgentFingerprint>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PublicNativeToolTarget {
    Agent { agent_id: AgentId },
    Component { component_id: Uuid },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct PublicTypedValue {
    pub graph: SchemaGraph,
    pub value: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PublicByteStreamRole {
    Stdin,
    Stdout,
    Stderr,
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
    pub byte_role: Option<PublicByteStreamRole>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_high_water: Option<PublicInputHighWater>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provisional_ref: Option<Uuid>,
    pub stream_token: String,
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

/// Bounded structural JSON; numbers retain their JSON representation until dispatch.
#[derive(Clone, PartialEq, Eq)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(evolution()))]
pub struct JsonMethodInput {
    value: String,
}

impl std::fmt::Debug for JsonMethodInput {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonMethodInput")
            .field("bytes", &self.value.len())
            .finish()
    }
}

impl JsonMethodInput {
    pub fn capture(bytes: &[u8]) -> Result<Self, PublicProtocolError> {
        let too_large = || {
            PublicProtocolError::new(
                PublicErrorCode::ResourceExhausted,
                "method input exceeds the 16 MiB logical value limit",
            )
        };
        if bytes.len() > MAX_LOGICAL_VALUE_SIZE {
            return Err(too_large());
        }
        let value = canonicalize_structural_json(bytes)?;
        if value.len() > MAX_LOGICAL_VALUE_SIZE {
            return Err(too_large());
        }
        Ok(Self { value })
    }

    pub fn as_str(&self) -> &str {
        &self.value
    }
}

impl Serialize for JsonMethodInput {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let raw: &serde_json::value::RawValue =
            serde_json::from_str(&self.value).map_err(serde::ser::Error::custom)?;
        raw.serialize(serializer)
    }
}

impl TryFrom<Value> for JsonMethodInput {
    type Error = PublicProtocolError;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        Self::capture(encode_json_value(&value)?.as_bytes())
    }
}

/// Client frames are decoded with [`decode_client_text`] and canonically encoded with
/// [`encode_client_text`], preserving the original method JSON independently of envelope controls.
#[derive(Clone, Debug, PartialEq, Serialize)]
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
        method_parameters: JsonMethodInput,
        selector: Box<InvocationSelector>,
        version: u8,
    },
    #[serde(rename = "toolStart")]
    ToolStart {
        #[serde(rename = "attemptId")]
        attempt_id: Uuid,
        application: String,
        environment: String,
        #[serde(rename = "idempotencyKey")]
        idempotency_key: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        #[serde(rename = "commandPath")]
        command_path: Vec<String>,
        target: PublicNativeToolTarget,
        input: Box<PublicTypedValue>,
        stdin: bool,
        stdout: bool,
        stderr: bool,
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
    #[serde(rename = "stream-end")]
    StreamEnd {
        channel: u32,
        direction: PublicStreamDirection,
        sequence: DecimalU64,
        outcome: PublicStreamOutcome,
        version: u8,
    },
}

impl<'de> Deserialize<'de> for PublicClientMessage {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Box::<serde_json::value::RawValue>::deserialize(deserializer)?;
        decode_client_text(raw.get().as_bytes()).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum PublicInvocationResult {
    None,
    Value {
        graph: SchemaGraph,
        value: Value,
    },
    ToolSuccess {
        result: Option<PublicTypedValue>,
    },
    ToolFailure {
        code: String,
        message: Option<String>,
        custom_error: Option<PublicTypedValue>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PublicStreamOutcome {
    Finished,
    Failed { error: PublicStreamError },
    ConsumerCancelled { details: Option<String> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum PublicStreamError {
    Failed { value: PublicTypedValue },
    Trapped { message: String },
    Cancelled,
    Abandoned,
    Interrupted { message: String },
    ResourceExhausted { message: String },
}

impl TryFrom<PublicTypedValue> for crate::schema::TypedSchemaValue {
    type Error = String;

    fn try_from(value: PublicTypedValue) -> Result<Self, Self::Error> {
        use crate::schema::public_json::{PublicStreamReferencePolicy, decode_public_schema_value};
        crate::schema::validation::validate_graph(&value.graph)
            .map_err(|errors| format!("invalid typed stream failure schema: {errors:?}"))?;
        let decoded = decode_public_schema_value(
            &value.graph,
            &value.graph.root,
            &value.value,
            PublicStreamReferencePolicy::None,
            |_, _| {
                Err(crate::schema::public_json::PublicSchemaValueError::new(
                    PublicErrorCode::UnsupportedValue,
                    "stream failures cannot contain stream references",
                ))
            },
        )
        .map_err(|error| error.to_string())?;
        Ok(Self::new(value.graph, decoded))
    }
}

impl TryFrom<crate::schema::TypedSchemaValue> for PublicTypedValue {
    type Error = String;

    fn try_from(value: crate::schema::TypedSchemaValue) -> Result<Self, Self::Error> {
        use crate::schema::public_json::{PublicSchemaValueError, encode_public_schema_value};
        crate::schema::validation::validate_graph(value.graph())
            .map_err(|errors| format!("invalid typed stream failure schema: {errors:?}"))?;
        let json =
            encode_public_schema_value(value.graph(), value.root_type(), value.value(), |_, _| {
                Err(PublicSchemaValueError::new(
                    PublicErrorCode::UnsupportedValue,
                    "stream failures cannot contain stream references",
                ))
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            graph: value.graph().clone(),
            value: json,
        })
    }
}

impl TryFrom<PublicStreamOutcome> for crate::model::durable_stream::StreamTerminal {
    type Error = String;

    fn try_from(value: PublicStreamOutcome) -> Result<Self, Self::Error> {
        use crate::model::durable_stream::StreamError;
        Ok(match value {
            PublicStreamOutcome::Finished => Self::Finished,
            PublicStreamOutcome::ConsumerCancelled { details } => {
                Self::ConsumerCancelled { details }
            }
            PublicStreamOutcome::Failed { error } => Self::Failed(match error {
                PublicStreamError::Failed { value } => {
                    StreamError::Failed(Box::new(value.try_into()?))
                }
                PublicStreamError::Trapped { message } => StreamError::Trapped(message),
                PublicStreamError::Cancelled => StreamError::Cancelled,
                PublicStreamError::Abandoned => StreamError::Abandoned,
                PublicStreamError::Interrupted { message } => StreamError::Interrupted(message),
                PublicStreamError::ResourceExhausted { message } => {
                    StreamError::ResourceExhausted(message)
                }
            }),
        })
    }
}

impl TryFrom<crate::model::durable_stream::StreamTerminal> for PublicStreamOutcome {
    type Error = String;

    fn try_from(value: crate::model::durable_stream::StreamTerminal) -> Result<Self, Self::Error> {
        use crate::model::durable_stream::{StreamError, StreamTerminal};
        Ok(match value {
            StreamTerminal::Finished => Self::Finished,
            StreamTerminal::ConsumerCancelled { details } => Self::ConsumerCancelled { details },
            StreamTerminal::Failed(error) => Self::Failed {
                error: match error {
                    StreamError::Failed(value) => PublicStreamError::Failed {
                        value: (*value).try_into()?,
                    },
                    StreamError::Trapped(message) => PublicStreamError::Trapped { message },
                    StreamError::Cancelled => PublicStreamError::Cancelled,
                    StreamError::Abandoned => PublicStreamError::Abandoned,
                    StreamError::Interrupted(message) => PublicStreamError::Interrupted { message },
                    StreamError::ResourceExhausted(message) => {
                        PublicStreamError::ResourceExhausted { message }
                    }
                },
            },
        })
    }
}

fn validate_public_terminal(outcome: &PublicStreamOutcome) -> Result<(), PublicProtocolError> {
    crate::model::durable_stream::StreamTerminal::try_from(outcome.clone())
        .map(|_| ())
        .map_err(|error| PublicProtocolError::new(PublicErrorCode::ValidationError, error))
}

#[cfg(feature = "full")]
mod terminal_protobuf {
    use crate::model::durable_stream::{StreamError, StreamTerminal, StreamTerminalAuthor};
    use golem_api_grpc::proto::golem::worker as proto;

    impl TryFrom<proto::StreamTerminal> for StreamTerminal {
        type Error = String;

        fn try_from(value: proto::StreamTerminal) -> Result<Self, Self::Error> {
            use proto::stream_terminal::Terminal;
            Ok(
                match value.terminal.ok_or("stream terminal has no outcome")? {
                    Terminal::Finished(_) => Self::Finished,
                    Terminal::ConsumerCancelled(value) => Self::ConsumerCancelled {
                        details: value.details,
                    },
                    Terminal::Failed(value) => {
                        Self::Failed(match value.error.ok_or("stream failure has no error")? {
                            proto::stream_error::Error::Failed(value) => {
                                let value: crate::schema::TypedSchemaValue = value.try_into()?;
                                crate::schema::validation::validate_graph(value.graph()).map_err(
                                    |errors| {
                                        format!("invalid typed stream failure schema: {errors:?}")
                                    },
                                )?;
                                crate::schema::validation::validate_value(
                                    value.graph(),
                                    value.root_type(),
                                    value.value(),
                                )
                                .map_err(|errors| {
                                    format!("invalid typed stream failure value: {errors:?}")
                                })?;
                                StreamError::Failed(Box::new(value))
                            }
                            proto::stream_error::Error::Trapped(message) => {
                                StreamError::Trapped(message)
                            }
                            proto::stream_error::Error::Cancelled(_) => StreamError::Cancelled,
                            proto::stream_error::Error::Abandoned(_) => StreamError::Abandoned,
                            proto::stream_error::Error::Interrupted(message) => {
                                StreamError::Interrupted(message)
                            }
                            proto::stream_error::Error::ResourceExhausted(message) => {
                                StreamError::ResourceExhausted(message)
                            }
                        })
                    }
                },
            )
        }
    }

    impl TryFrom<StreamTerminal> for proto::StreamTerminal {
        type Error = String;

        fn try_from(value: StreamTerminal) -> Result<Self, Self::Error> {
            use proto::stream_terminal::Terminal;
            Ok(Self {
                terminal: Some(match value {
                    StreamTerminal::Finished => Terminal::Finished(Default::default()),
                    StreamTerminal::ConsumerCancelled { details } => {
                        Terminal::ConsumerCancelled(proto::StreamConsumerCancelled { details })
                    }
                    StreamTerminal::Failed(error) => Terminal::Failed(proto::StreamError {
                        error: Some(match error {
                            StreamError::Failed(value) => {
                                crate::schema::validation::validate_graph(value.graph()).map_err(
                                    |errors| {
                                        format!("invalid typed stream failure schema: {errors:?}")
                                    },
                                )?;
                                crate::schema::validation::validate_value(
                                    value.graph(),
                                    value.root_type(),
                                    value.value(),
                                )
                                .map_err(|errors| {
                                    format!("invalid typed stream failure value: {errors:?}")
                                })?;
                                proto::stream_error::Error::Failed((*value).try_into()?)
                            }
                            StreamError::Trapped(message) => {
                                proto::stream_error::Error::Trapped(message)
                            }
                            StreamError::Cancelled => {
                                proto::stream_error::Error::Cancelled(Default::default())
                            }
                            StreamError::Abandoned => {
                                proto::stream_error::Error::Abandoned(Default::default())
                            }
                            StreamError::Interrupted(message) => {
                                proto::stream_error::Error::Interrupted(message)
                            }
                            StreamError::ResourceExhausted(message) => {
                                proto::stream_error::Error::ResourceExhausted(message)
                            }
                        }),
                    }),
                }),
            })
        }
    }

    impl From<StreamTerminalAuthor> for proto::StreamTerminalAuthor {
        fn from(value: StreamTerminalAuthor) -> Self {
            match value {
                StreamTerminalAuthor::Guest => Self::Guest,
                StreamTerminalAuthor::Host => Self::Host,
                StreamTerminalAuthor::Protocol => Self::Protocol,
            }
        }
    }

    impl TryFrom<proto::StreamTerminalAuthor> for StreamTerminalAuthor {
        type Error = String;

        fn try_from(value: proto::StreamTerminalAuthor) -> Result<Self, Self::Error> {
            match value {
                proto::StreamTerminalAuthor::Guest => Ok(Self::Guest),
                proto::StreamTerminalAuthor::Host => Ok(Self::Host),
                proto::StreamTerminalAuthor::Protocol => Ok(Self::Protocol),
                proto::StreamTerminalAuthor::Unspecified => {
                    Err("stream terminal author is unspecified".into())
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum PublicInvocationOutcome {
    Success,
    Failure {
        code: PublicErrorCode,
        message: String,
        /// What the failed invocation wrote to stderr, for a failure of the agent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stderr: Option<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum PublicServerMessage {
    #[serde(rename = "invocationReady")]
    InvocationReady {
        #[serde(rename = "attemptId")]
        attempt_id: Uuid,
        #[serde(rename = "agentId")]
        agent_id: AgentId,
        #[serde(rename = "idempotencyKey")]
        idempotency_key: InvocationId,
        mappings: Vec<PublicStreamMapping>,
        #[serde(rename = "sessionToken")]
        session_token: String,
        version: u8,
    },
    #[serde(rename = "invocationAccepted")]
    InvocationAccepted {
        #[serde(rename = "attemptId")]
        attempt_id: Uuid,
        #[serde(rename = "agentId")]
        agent_id: AgentId,
        #[serde(rename = "idempotencyKey")]
        idempotency_key: InvocationId,
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
        /// What the failed invocation wrote to stderr, for a rejection caused by a failure of
        /// the agent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stderr: Option<String>,
        version: u8,
    },
    #[serde(rename = "invocationResult")]
    InvocationResult {
        mappings: Vec<PublicStreamMapping>,
        result: Box<PublicInvocationResult>,
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
    #[serde(rename = "stream-end")]
    StreamEnd {
        channel: u32,
        direction: PublicStreamDirection,
        #[serde(
            default,
            rename = "cursorToken",
            skip_serializing_if = "Option::is_none"
        )]
        cursor_token: Option<String>,
        outcome: PublicStreamOutcome,
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
    let canonical = canonicalize_structural_json(bytes)?;
    let StrictRawJsonObject(mut raw_fields) =
        serde_json::from_str(&canonical).map_err(|_| invalid_client_message())?;
    let method_parameters = if raw_fields.get("type").is_some_and(|value| {
        serde_json::from_str::<String>(value.get()).is_ok_and(|kind| kind == "invocationStart")
    }) {
        raw_fields.remove("methodParameters")
    } else {
        None
    };
    let controls = serde_json::to_vec(&raw_fields).map_err(|_| invalid_client_message())?;
    let value: Value = serde_json::from_slice(&controls).map_err(|_| invalid_client_message())?;
    check_version(&value)?;
    prevalidate_decimal_fields(&value)?;
    prevalidate_uuid_fields(&value)?;
    let Value::Object(mut fields) = value else {
        return Err(invalid_client_message());
    };
    let kind: String = take_client_field(&mut fields, "type")?;
    let message = match kind.as_str() {
        "invocationStart" => PublicClientMessage::InvocationStart {
            attempt_id: take_client_field(&mut fields, "attemptId")?,
            config: take_client_field(&mut fields, "config")?,
            idempotency_key: take_client_field(&mut fields, "idempotencyKey")?,
            method_parameters: JsonMethodInput::capture(
                method_parameters
                    .ok_or_else(invalid_client_message)?
                    .get()
                    .as_bytes(),
            )?,
            selector: take_client_field(&mut fields, "selector")?,
            version: take_client_field(&mut fields, "version")?,
        },
        "toolStart" => PublicClientMessage::ToolStart {
            attempt_id: take_client_field(&mut fields, "attemptId")?,
            application: take_client_field(&mut fields, "application")?,
            environment: take_client_field(&mut fields, "environment")?,
            idempotency_key: take_client_field(&mut fields, "idempotencyKey")?,
            tool_name: take_client_field(&mut fields, "toolName")?,
            command_path: take_client_field(&mut fields, "commandPath")?,
            target: take_client_field(&mut fields, "target")?,
            input: take_client_field(&mut fields, "input")?,
            stdin: take_client_field(&mut fields, "stdin")?,
            stdout: take_client_field(&mut fields, "stdout")?,
            stderr: take_client_field(&mut fields, "stderr")?,
            version: take_client_field(&mut fields, "version")?,
        },
        "resumeAttach" => PublicClientMessage::ResumeAttach {
            attempt_id: take_client_field(&mut fields, "attemptId")?,
            operation: take_client_field(&mut fields, "operation")?,
            output_cursors: take_client_field(&mut fields, "outputCursors")?,
            session_token: take_client_field(&mut fields, "sessionToken")?,
            version: take_client_field(&mut fields, "version")?,
        },
        "inputStreamItem" => PublicClientMessage::InputStreamItem {
            channel: take_client_field(&mut fields, "channel")?,
            sequence: take_client_field(&mut fields, "sequence")?,
            value: take_client_field(&mut fields, "value")?,
            version: take_client_field(&mut fields, "version")?,
        },
        "stream-end" => PublicClientMessage::StreamEnd {
            channel: take_client_field(&mut fields, "channel")?,
            direction: take_client_field(&mut fields, "direction")?,
            sequence: take_client_field(&mut fields, "sequence")?,
            outcome: take_client_field(&mut fields, "outcome")?,
            version: take_client_field(&mut fields, "version")?,
        },
        _ => return Err(invalid_client_message()),
    };
    if !fields.is_empty() {
        return Err(invalid_client_message());
    }
    validate_client_message(&message)?;
    Ok(message)
}

fn invalid_client_message() -> PublicProtocolError {
    PublicProtocolError::new(PublicErrorCode::MalformedMessage, "invalid client message")
}

fn take_client_field<T: DeserializeOwned>(
    fields: &mut serde_json::Map<String, Value>,
    name: &str,
) -> Result<T, PublicProtocolError> {
    serde_json::from_value(fields.remove(name).ok_or_else(invalid_client_message)?)
        .map_err(|_| invalid_client_message())
}

pub fn encode_client_text(message: &PublicClientMessage) -> Result<String, PublicProtocolError> {
    let PublicClientMessage::InvocationStart {
        attempt_id,
        config,
        idempotency_key,
        method_parameters,
        selector,
        version,
    } = message
    else {
        return encode_text(message);
    };
    let controls = encode_json_value(&serde_json::json!({
        "attemptId": attempt_id,
        "config": config,
        "idempotencyKey": idempotency_key,
        "selector": selector,
        "type": "invocationStart",
        "version": version,
    }))?;
    let StrictRawJsonObject(mut fields) = serde_json::from_str(&controls).map_err(|_| {
        PublicProtocolError::new(
            PublicErrorCode::InternalError,
            "failed to encode client message",
        )
    })?;
    let input = serde_json::from_str(method_parameters.as_str()).map_err(|_| {
        PublicProtocolError::new(
            PublicErrorCode::InternalError,
            "invalid captured method input",
        )
    })?;
    fields.insert("methodParameters".to_string(), input);
    let encoded = serde_json::to_string(&fields).map_err(|_| {
        PublicProtocolError::new(
            PublicErrorCode::InternalError,
            "failed to encode client message",
        )
    })?;
    validate_message_size(encoded.len())?;
    Ok(encoded)
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
    let canonical = canonicalize_structural_json(bytes)?;
    serde_json::from_str(&canonical)
        .map_err(|_| PublicProtocolError::new(PublicErrorCode::MalformedMessage, "invalid JSON"))
}

/// Checks JSON structure without interpreting number tokens as a numeric type.
pub fn canonicalize_structural_json(bytes: &[u8]) -> Result<String, PublicProtocolError> {
    let raw: &serde_json::value::RawValue = serde_json::from_slice(bytes)
        .map_err(|_| PublicProtocolError::new(PublicErrorCode::MalformedMessage, "invalid JSON"))?;
    let mut canonical = String::with_capacity(bytes.len());
    write_structural_json(raw, &mut canonical, 0)?;
    Ok(canonical)
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
        PublicClientMessage::ToolStart {
            attempt_id,
            application,
            environment,
            idempotency_key,
            tool_name,
            command_path,
            target,
            ..
        } => {
            validate_uuid_v4(*attempt_id, "attempt ID")?;
            if application.is_empty()
                || environment.is_empty()
                || tool_name.is_empty()
                || command_path.iter().any(String::is_empty)
                || idempotency_key.is_empty()
                || idempotency_key.len() > MAX_IDEMPOTENCY_KEY_SIZE
            {
                return Err(PublicProtocolError::new(
                    PublicErrorCode::ValidationError,
                    "tool start names, command path segments, and idempotency key must not be empty",
                ));
            }
            if let PublicNativeToolTarget::Agent { agent_id } = target
                && (!agent_id.is_issued() || agent_id.type_id().is_none())
            {
                return Err(PublicProtocolError::new(
                    PublicErrorCode::ValidationError,
                    "tool target agent ID must be a version 8 agent ID with an agent type tag",
                ));
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
        PublicClientMessage::InputStreamItem { channel, .. } => validate_channel(*channel)?,
        PublicClientMessage::StreamEnd {
            channel,
            direction,
            outcome,
            ..
        } => {
            validate_channel(*channel)?;
            let consumer = matches!(outcome, PublicStreamOutcome::ConsumerCancelled { .. });
            if consumer != (*direction == PublicStreamDirection::Output) {
                return Err(PublicProtocolError::new(
                    PublicErrorCode::ProtocolError,
                    "client input stream ends must be producer terminals; output stream ends must be consumer cancellations",
                ));
            }
            validate_public_terminal(outcome)?;
        }
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
            agent_id,
            mappings,
            session_token,
            ..
        }
        | PublicServerMessage::InvocationReady {
            attempt_id,
            agent_id,
            mappings,
            session_token,
            ..
        } => {
            validate_uuid_v4(*attempt_id, "attempt ID")?;
            if !agent_id.is_issued() {
                return Err(PublicProtocolError::new(
                    PublicErrorCode::MalformedMessage,
                    "accepted agent ID must be a version 8 agent ID",
                ));
            }
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
        PublicServerMessage::StreamEnd {
            channel,
            cursor_token,
            outcome,
            ..
        } => {
            validate_channel(*channel)?;
            validate_public_terminal(outcome)?;
            if let Some(cursor) = cursor_token {
                validate_token_text(cursor)?;
            }
        }
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
    let mut byte_roles = Vec::new();
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
        if let Some(role) = mapping.byte_role {
            if byte_roles.contains(&role) {
                return Err(PublicProtocolError::new(
                    PublicErrorCode::StreamConflict,
                    "tool byte stream roles must be distinct",
                ));
            }
            byte_roles.push(role);
            let expected_direction = match role {
                PublicByteStreamRole::Stdin => PublicStreamDirection::Input,
                PublicByteStreamRole::Stdout | PublicByteStreamRole::Stderr => {
                    PublicStreamDirection::Output
                }
            };
            if mapping.direction != expected_direction {
                return Err(PublicProtocolError::new(
                    PublicErrorCode::StreamConflict,
                    "tool byte stream role does not match its direction",
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

fn write_structural_json(
    raw: &serde_json::value::RawValue,
    output: &mut String,
    depth: usize,
) -> Result<(), PublicProtocolError> {
    if depth > MAX_JSON_DEPTH {
        return Err(PublicProtocolError::new(
            PublicErrorCode::ResourceExhausted,
            "JSON nesting exceeds 64 levels",
        ));
    }
    let invalid = |_| PublicProtocolError::new(PublicErrorCode::MalformedMessage, "invalid JSON");
    let text = raw.get();
    match text.as_bytes().first() {
        Some(b'[') => {
            let values: Vec<&serde_json::value::RawValue> =
                serde_json::from_str(text).map_err(invalid)?;
            if values.len() > MAX_COLLECTION_SIZE {
                return Err(PublicProtocolError::new(
                    PublicErrorCode::ResourceExhausted,
                    "JSON collection exceeds 100000 elements",
                ));
            }
            output.push('[');
            for (index, value) in values.into_iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                write_structural_json(value, output, depth + 1)?;
            }
            output.push(']');
        }
        Some(b'{') => {
            let StrictRawJsonObject(values) = serde_json::from_str(text).map_err(invalid)?;
            if values.len() > MAX_COLLECTION_SIZE {
                return Err(PublicProtocolError::new(
                    PublicErrorCode::ResourceExhausted,
                    "JSON object exceeds 100000 members",
                ));
            }
            output.push('{');
            for (index, (name, value)) in values.into_iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                output.push_str(&serde_json::to_string(&name).map_err(invalid)?);
                output.push(':');
                write_structural_json(value, output, depth + 1)?;
            }
            output.push('}');
        }
        Some(b'"') => {
            let value: String = serde_json::from_str(text).map_err(invalid)?;
            output.push_str(&serde_json::to_string(&value).map_err(invalid)?);
        }
        _ => output.push_str(text),
    }
    Ok(())
}

struct StrictRawJsonObject<'a>(BTreeMap<String, &'a serde_json::value::RawValue>);

impl<'de> Deserialize<'de> for StrictRawJsonObject<'de> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(StrictRawJsonObjectVisitor)
    }
}

struct StrictRawJsonObjectVisitor;

impl<'de> Visitor<'de> for StrictRawJsonObjectVisitor {
    type Value = StrictRawJsonObject<'de>;

    fn expecting(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a JSON object without duplicate members")
    }

    fn visit_map<A>(self, mut object: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = BTreeMap::new();
        while let Some((key, value)) =
            object.next_entry::<String, &serde_json::value::RawValue>()?
        {
            if values.insert(key, value).is_some() {
                return Err(serde::de::Error::custom("duplicate JSON object member"));
            }
        }
        Ok(StrictRawJsonObject(values))
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "full")]
    use super::new_durable_stream_session_id;
    use super::{
        BinaryMessageKind, InvocationSelector, MAX_WEBSOCKET_MESSAGE_SIZE, PublicAgentTarget,
        PublicByteStreamRole, PublicClientMessage, PublicErrorCode, PublicInvocationResult,
        PublicNativeToolTarget, PublicServerMessage, PublicTypedValue, decode_binary_message,
        decode_client_text, decode_server_text, encode_client_text, encode_text,
        validate_durable_stream_session_id, validate_message_size,
    };
    use crate::schema::{SchemaGraph, SchemaType};
    use serde::Deserialize;
    use serde_json::Value;
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
                "client" => {
                    encode_client_text(&decode_client_text(vector.canonical.as_bytes()).unwrap())
                }
                "server" => encode_text(&decode_server_text(vector.canonical.as_bytes()).unwrap()),
                other => panic!("unexpected fixture direction {other}"),
            }
            .unwrap();
            assert_eq!(encoded, vector.canonical);
        }
    }

    fn invocation_start_json(input: &str) -> String {
        r#"{"attemptId":"11111111-1111-4111-8111-111111111111","config":[],"idempotencyKey":"raw-input","methodParameters":INPUT,"selector":{"agentType":"agent","application":"app","environment":"env","method":"run","target":{"constructorParameters":{},"kind":"key"}},"type":"invocationStart","version":1}"#
            .replace("INPUT", input)
    }

    #[test]
    fn client_method_json_retains_number_lexemes_without_changing_control_numbers() {
        for input in [
            "1e400",
            "-1e999",
            "1.00000000000000001",
            "1",
            "1.0",
            "1e0",
            "-0",
            "-0.0",
            "null",
            r#"{"a":1e400,"b":[1.00000000000000001,-0.0]}"#,
        ] {
            let encoded = invocation_start_json(input);
            let message = decode_client_text(encoded.as_bytes()).unwrap();
            let PublicClientMessage::InvocationStart {
                method_parameters, ..
            } = &message
            else {
                panic!("expected invocation start");
            };
            assert_eq!(method_parameters.as_str(), input);
            assert_eq!(encode_client_text(&message).unwrap(), encoded);
            assert_eq!(
                decode_client_text(encode_client_text(&message).unwrap().as_bytes()).unwrap(),
                message
            );
            assert_eq!(
                serde_json::from_reader::<_, PublicClientMessage>(std::io::Cursor::new(
                    encoded.as_bytes()
                ))
                .unwrap(),
                message
            );
            assert_eq!(
                decode_client_text(serde_json::to_string(&message).unwrap().as_bytes()).unwrap(),
                message
            );
        }
        let control = PublicClientMessage::InputStreamItem {
            channel: 1,
            sequence: super::DecimalU64(0),
            value: serde_json::json!(-0.0),
            version: 1,
        };
        assert_eq!(
            encode_client_text(&control).unwrap(),
            r#"{"channel":1,"sequence":"0","type":"inputStreamItem","value":-0,"version":1}"#
        );
        let encoded = invocation_start_json("-0.0").replace(
            r#""config":[]"#,
            r#""config":[{"path":["x"],"value":-0.0}]"#,
        );
        assert!(
            encode_client_text(&decode_client_text(encoded.as_bytes()).unwrap())
                .unwrap()
                .contains(r#""value":-0}"#)
        );
        for encoded in [
            invocation_start_json("0").replace(r#""version":1"#, r#""version":1e400"#),
            invocation_start_json("0").replace(
                r#""config":[]"#,
                r#""config":[{"path":["x"],"value":1e400}]"#,
            ),
            invocation_start_json("0").replace(
                r#""constructorParameters":{}"#,
                r#""constructorParameters":1e400"#,
            ),
            r#"{"channel":1,"sequence":"0","type":"inputStreamItem","value":1e400,"version":1}"#
                .into(),
        ] {
            assert_eq!(
                decode_client_text(encoded.as_bytes()).unwrap_err().code,
                PublicErrorCode::MalformedMessage
            );
        }
    }

    #[test]
    fn raw_client_method_input_preserves_structural_and_envelope_limits() {
        let input_at_depth_limit = format!("{}0{}", "[".repeat(63), "]".repeat(63));
        assert!(
            decode_client_text(invocation_start_json(&input_at_depth_limit).as_bytes()).is_ok()
        );
        let too_deep = format!("[{input_at_depth_limit}]");
        assert_eq!(
            decode_client_text(invocation_start_json(&too_deep).as_bytes())
                .unwrap_err()
                .code,
            PublicErrorCode::ResourceExhausted
        );
        for input in [r#"{"a":1,"\u0061":2}"#, r#"{"x":{"a":1,"a":2}}"#] {
            assert_eq!(
                decode_client_text(invocation_start_json(input).as_bytes())
                    .unwrap_err()
                    .code,
                PublicErrorCode::MalformedMessage
            );
        }
        for duplicate in [
            invocation_start_json("0").replace(r#""version":1"#, r#""version":1,"\u0076ersion":1"#),
            invocation_start_json("0").replace(
                r#""config":[]"#,
                r#""config":[{"path":["x"],"value":{"a":1,"\u0061":2}}]"#,
            ),
        ] {
            assert_eq!(
                decode_client_text(duplicate.as_bytes()).unwrap_err().code,
                PublicErrorCode::MalformedMessage
            );
        }
        let missing = invocation_start_json("null").replace(r#""methodParameters":null,"#, "");
        assert_eq!(
            decode_client_text(missing.as_bytes()).unwrap_err().code,
            PublicErrorCode::MalformedMessage
        );
        let oversized_input = format!("\"{}\"", "x".repeat(super::MAX_LOGICAL_VALUE_SIZE - 1));
        assert_eq!(
            decode_client_text(invocation_start_json(&oversized_input).as_bytes())
                .unwrap_err()
                .code,
            PublicErrorCode::ResourceExhausted
        );
        let at_limit = format!("\"{}\"", "x".repeat(super::MAX_LOGICAL_VALUE_SIZE - 2));
        let mut message = decode_client_text(invocation_start_json(&at_limit).as_bytes()).unwrap();
        assert!(encode_client_text(&message).is_ok());
        if let PublicClientMessage::InvocationStart { config, .. } = &mut message {
            config.push(super::PublicConfigEntry {
                path: vec!["x".into()],
                value: Value::String("x".repeat(super::MAX_LOGICAL_VALUE_SIZE)),
            });
        }
        assert_eq!(
            encode_client_text(&message).unwrap_err().code,
            PublicErrorCode::ResourceExhausted
        );
        let at_collection_limit = format!("[{}]", vec!["0"; super::MAX_COLLECTION_SIZE].join(","));
        assert!(decode_client_text(invocation_start_json(&at_collection_limit).as_bytes()).is_ok());
        let too_many = at_collection_limit.replacen('[', "[0,", 1);
        assert_eq!(
            decode_client_text(invocation_start_json(&too_many).as_bytes())
                .unwrap_err()
                .code,
            PublicErrorCode::ResourceExhausted
        );
    }

    #[test]
    fn accepted_message_requires_an_issued_agent_id_of_any_tag() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../golem-client/tests/fixtures/stream-session-v1/json-messages.json"
        )))
        .unwrap();
        let canonical = fixture["vectors"]
            .as_array()
            .unwrap()
            .iter()
            .find(|vector| vector["name"] == "invocation-accepted")
            .unwrap()["canonical"]
            .as_str()
            .unwrap();
        let with_agent_id = |agent_id: &str| {
            let mut message: serde_json::Value = serde_json::from_str(canonical).unwrap();
            message["agentId"] = serde_json::Value::String(agent_id.to_string());
            decode_server_text(&serde_json::to_vec(&message).unwrap())
        };
        assert!(with_agent_id("00000000-0000-8000-8000-000000000004").is_ok());
        for not_issued in [
            "00000007-0000-4000-8000-000000000004",
            "00000007-0000-8000-c000-000000000004",
        ] {
            assert_eq!(
                with_agent_id(not_issued).unwrap_err().code,
                PublicErrorCode::MalformedMessage,
                "{not_issued}"
            );
        }
    }

    #[test]
    fn strict_json_rejects_duplicate_and_unknown_fields() {
        let duplicate = br#"{"channel":1,"direction":"input","outcome":{"kind":"finished"},"sequence":"0","type":"stream-end","type":"stream-end","version":1}"#;
        assert_eq!(
            decode_client_text(duplicate).unwrap_err().code,
            PublicErrorCode::MalformedMessage
        );
        let unknown =
            br#"{"channel":1,"direction":"input","extra":true,"outcome":{"kind":"finished"},"sequence":"0","type":"stream-end","version":1}"#;
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
        let bytes = br#"{"channel":1,"direction":"input","outcome":{"kind":"finished"},"sequence":"0","type":"stream-end","version":2}"#;
        assert_eq!(
            decode_client_text(bytes).unwrap_err().code,
            PublicErrorCode::UnsupportedVersion
        );
    }

    #[test]
    fn malformed_fixture_invalid_attempt_uuid_has_the_frozen_error_code() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../golem-client/tests/fixtures/stream-session-v1/malformed.json"
        )))
        .unwrap();
        let vector = fixture["vectors"]
            .as_array()
            .unwrap()
            .iter()
            .find(|vector| vector["name"] == "invalid-attempt-uuid")
            .unwrap();
        assert_eq!(vector["expectedCode"], "validation-error");
        assert_eq!(
            decode_client_text(vector["input"].as_str().unwrap().as_bytes())
                .unwrap_err()
                .code,
            PublicErrorCode::ValidationError
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
        let client =
            br#"{"channel":1,"value":17,"sequence":"0","type":"inputStreamItem","version":1}"#;
        assert!(matches!(
            decode_client_text(client).unwrap(),
            PublicClientMessage::InputStreamItem { .. }
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
    fn stream_end_json_enforces_direction_and_outcome_without_an_author() {
        for direction in ["input", "output"] {
            for (outcome, consumer) in [
                (serde_json::json!({"kind":"finished"}), false),
                (
                    serde_json::json!({"kind":"failed", "error":{"kind":"interrupted", "message":"source unavailable"}}),
                    false,
                ),
                (
                    serde_json::json!({"kind":"consumer-cancelled", "details":null}),
                    true,
                ),
            ] {
                let value = serde_json::json!({
                    "channel":1, "direction":direction, "sequence":"0", "outcome":outcome,
                    "type":"stream-end", "version":1,
                });
                let text = serde_json::to_vec(&value).unwrap();
                assert_eq!(
                    decode_client_text(&text).is_ok(),
                    consumer == (direction == "output")
                );
                assert!(decode_server_text(&text).is_ok());
            }
        }
        for outcome in [
            serde_json::json!({"kind":"unknown"}),
            serde_json::json!({"kind":"failed", "error":{"kind":"unknown"}}),
        ] {
            let value = serde_json::json!({
                "channel":1, "direction":"input", "sequence":"0", "outcome":outcome,
                "type":"stream-end", "version":1,
            });
            assert!(decode_client_text(&serde_json::to_vec(&value).unwrap()).is_err());
        }
    }

    fn terminal_values() -> Vec<crate::model::durable_stream::StreamTerminal> {
        use crate::model::durable_stream::{StreamError, StreamTerminal};
        use crate::schema::IntoTypedSchemaValue;
        vec![
            StreamTerminal::Finished,
            StreamTerminal::Failed(StreamError::Failed(Box::new(
                42u64.into_typed_schema_value().unwrap(),
            ))),
            StreamTerminal::Failed(StreamError::Trapped("trap".into())),
            StreamTerminal::Failed(StreamError::Cancelled),
            StreamTerminal::Failed(StreamError::Abandoned),
            StreamTerminal::Failed(StreamError::Interrupted("source unavailable".into())),
            StreamTerminal::Failed(StreamError::ResourceExhausted("quota".into())),
            StreamTerminal::ConsumerCancelled { details: None },
            StreamTerminal::ConsumerCancelled {
                details: Some("reader dropped".into()),
            },
        ]
    }

    #[test]
    fn typed_stream_terminal_public_round_trips_use_canonical_schema_values() {
        use super::{
            DecimalU64, INVOCATION_SESSION_VERSION, PublicStreamDirection, PublicStreamOutcome,
        };
        use crate::model::durable_stream::StreamTerminal;
        for terminal in terminal_values() {
            let public = PublicStreamOutcome::try_from(terminal.clone()).unwrap();
            let json = serde_json::to_vec(&public).unwrap();
            let decoded: PublicStreamOutcome = serde_json::from_slice(&json).unwrap();
            assert_eq!(StreamTerminal::try_from(decoded).unwrap(), terminal);
            for direction in [PublicStreamDirection::Input, PublicStreamDirection::Output] {
                let message = PublicServerMessage::StreamEnd {
                    channel: 7,
                    direction,
                    cursor_token: None,
                    outcome: public.clone(),
                    sequence: DecimalU64(13),
                    version: INVOCATION_SESSION_VERSION,
                };
                let json = encode_text(&message).unwrap();
                assert_eq!(decode_server_text(json.as_bytes()).unwrap(), message);
            }
        }
        use crate::schema::IntoTypedSchemaValue;
        let mut typed =
            PublicTypedValue::try_from(42u64.into_typed_schema_value().unwrap()).unwrap();
        typed.value = serde_json::Value::Null;
        let value = serde_json::json!({
            "channel":1, "direction":"input", "sequence":"0",
            "outcome":{"kind":"failed", "error":{"kind":"failed", "value":typed}},
            "type":"stream-end", "version":1,
        });
        assert_eq!(
            decode_client_text(&serde_json::to_vec(&value).unwrap())
                .unwrap_err()
                .code,
            PublicErrorCode::ValidationError
        );
    }

    #[cfg(feature = "full")]
    #[test]
    fn typed_stream_terminal_proto_round_trips_and_authors() {
        use crate::model::durable_stream::{StreamTerminal, StreamTerminalAuthor};
        use golem_api_grpc::proto::golem::worker as proto;
        for terminal in terminal_values() {
            let wire = proto::StreamTerminal::try_from(terminal.clone()).unwrap();
            assert_eq!(StreamTerminal::try_from(wire).unwrap(), terminal);
        }
        assert!(StreamTerminal::try_from(proto::StreamTerminal::default()).is_err());
        for author in [
            StreamTerminalAuthor::Guest,
            StreamTerminalAuthor::Host,
            StreamTerminalAuthor::Protocol,
        ] {
            assert_eq!(
                StreamTerminalAuthor::try_from(proto::StreamTerminalAuthor::from(author)).unwrap(),
                author
            );
        }
        assert!(StreamTerminalAuthor::try_from(proto::StreamTerminalAuthor::Unspecified).is_err());
    }

    #[test]
    fn server_input_ack_rejects_reserved_zero_channel() {
        let input = br#"{"channel":0,"highestContiguousSequence":"0","mappings":[],"terminal":false,"type":"inputStreamAck","version":1}"#;
        let error = decode_server_text(input).expect_err("channel zero must remain reserved");
        assert_eq!(error.code, PublicErrorCode::InvalidChannel);
    }

    #[test]
    fn session_selectors_require_an_explicit_key_or_id_target() {
        let id = "00000007-0000-8000-8000-000000000001";
        let selector = serde_json::json!({
            "agentType": "agent", "application": "app", "environment": "env", "method": "run",
            "target": {"kind": "key", "constructorParameters": id}
        });
        let key: InvocationSelector = serde_json::from_value(selector.clone()).unwrap();
        assert_eq!(
            key.target,
            PublicAgentTarget::Key {
                constructor_parameters: Value::String(id.into())
            }
        );
        let mut by_id = selector.clone();
        by_id["target"] = serde_json::json!({"kind": "id", "agentId": id});
        let parsed: InvocationSelector = serde_json::from_value(by_id.clone()).unwrap();
        assert_eq!(
            parsed.target,
            PublicAgentTarget::Id {
                agent_id: id.parse().unwrap(),
                expected_fingerprint: None
            }
        );
        assert_eq!(serde_json::to_value(parsed).unwrap(), by_id);
        for invalid in [
            serde_json::json!({"kind": "id", "agentId": id, "constructorParameters": {}}),
            serde_json::json!({"kind": "key", "constructorParameters": {}, "agentId": id}),
            serde_json::json!({"kind": "id", "agentId": "not-an-id"}),
            serde_json::json!({"kind": "key"}),
        ] {
            let mut invalid_selector = selector.clone();
            invalid_selector["target"] = invalid;
            assert!(serde_json::from_value::<InvocationSelector>(invalid_selector).is_err());
        }
        let mut legacy = selector;
        legacy.as_object_mut().unwrap().remove("target");
        legacy["constructorParameters"] = serde_json::json!({});
        assert!(serde_json::from_value::<InvocationSelector>(legacy).is_err());
    }

    #[test]
    fn client_attempt_ids_must_be_uuid_v4() {
        let input = br#"{"attemptId":"00000000-0000-0000-0000-000000000000","config":[],"idempotencyKey":"key","methodParameters":{},"selector":{"agentType":"agent","application":"app","target":{"kind":"key","constructorParameters":{}},"environment":"env","method":"run"},"type":"invocationStart","version":1}"#;

        assert!(
            decode_client_text(input).is_err(),
            "the public v1 contract requires attempt IDs to be UUIDv4 values"
        );
    }

    #[test]
    fn client_attempt_ids_must_use_the_rfc4122_variant() {
        let input = br#"{"attemptId":"00000000-0000-4000-0000-000000000000","config":[],"idempotencyKey":"key","methodParameters":{},"selector":{"agentType":"agent","application":"app","target":{"kind":"key","constructorParameters":{}},"environment":"env","method":"run"},"type":"invocationStart","version":1}"#;

        assert!(
            decode_client_text(input).is_err(),
            "UUIDv4 requires the RFC 4122 variant as well as the version-4 nibble"
        );
    }

    #[test]
    fn native_tool_start_round_trips_camel_case_fields_and_allows_root_command() {
        let message = PublicClientMessage::ToolStart {
            attempt_id: uuid::Uuid::new_v4(),
            application: "app".to_string(),
            environment: "env".to_string(),
            idempotency_key: "key".to_string(),
            tool_name: "tool".to_string(),
            command_path: Vec::new(),
            target: PublicNativeToolTarget::Component {
                component_id: uuid::Uuid::new_v4(),
            },
            input: Box::new(PublicTypedValue {
                graph: SchemaGraph::anonymous(SchemaType::u8()),
                value: serde_json::json!({"kind":"u8","value":7}),
            }),
            stdin: true,
            stdout: true,
            stderr: true,
            version: 1,
        };
        let encoded = encode_text(&message).unwrap();
        assert!(encoded.contains("\"commandPath\""));
        assert!(encoded.contains("\"componentId\""));
        assert!(encoded.contains("\"graph\""));
        assert!(!encoded.contains("\"schema\""));
        assert!(matches!(
            decode_client_text(encoded.as_bytes()).unwrap(),
            PublicClientMessage::ToolStart { command_path, .. } if command_path.is_empty()
        ));
    }

    #[test]
    fn method_result_carries_a_graph_and_value_envelope() {
        let message = PublicServerMessage::InvocationResult {
            mappings: Vec::new(),
            result: Box::new(PublicInvocationResult::Value {
                graph: SchemaGraph::anonymous(SchemaType::string()),
                value: serde_json::json!({"kind":"string","value":"done"}),
            }),
            version: 1,
        };

        let encoded = encode_text(&message).unwrap();
        assert!(encoded.contains("\"graph\""));
        assert!(!encoded.contains("\"schema\""));
        assert_eq!(decode_server_text(encoded.as_bytes()).unwrap(), message);
    }

    #[test]
    fn stderr_byte_role_has_an_explicit_public_identity() {
        let encoded = serde_json::to_value(PublicByteStreamRole::Stderr).unwrap();
        assert_eq!(encoded, serde_json::json!("stderr"));
        assert_eq!(
            serde_json::from_value::<PublicByteStreamRole>(encoded).unwrap(),
            PublicByteStreamRole::Stderr
        );
    }

    #[test]
    fn public_mappings_reject_duplicate_byte_roles() {
        use super::{PublicStreamDirection, PublicStreamMapping, validate_mappings};

        let output = |channel, stream_token: &str, byte_role| PublicStreamMapping {
            channel,
            direction: PublicStreamDirection::Output,
            byte_role: Some(byte_role),
            input_high_water: None,
            provisional_ref: None,
            stream_token: stream_token.to_string(),
        };
        assert!(
            validate_mappings(&[
                output(1, "first", PublicByteStreamRole::Stderr),
                output(2, "second", PublicByteStreamRole::Stderr),
            ])
            .is_err(),
            "one acceptance must not assign the stderr role to two streams"
        );
    }

    #[test]
    fn public_mappings_reject_byte_roles_with_the_wrong_direction() {
        use super::{
            DecimalU64, PublicInputHighWater, PublicStreamDirection, PublicStreamMapping,
            validate_mappings,
        };

        let input_with_stdout_role = PublicStreamMapping {
            channel: 3,
            direction: PublicStreamDirection::Input,
            byte_role: Some(PublicByteStreamRole::Stdout),
            input_high_water: Some(PublicInputHighWater {
                sequence: DecimalU64(0),
                terminal: false,
            }),
            provisional_ref: None,
            stream_token: "third".to_string(),
        };
        assert!(
            validate_mappings(&[input_with_stdout_role]).is_err(),
            "stdout and stderr roles are output-only"
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

    #[cfg(feature = "full")]
    #[test]
    fn durable_stream_session_ids_are_uuids() {
        let first = new_durable_stream_session_id();
        let second = new_durable_stream_session_id();
        assert_ne!(first, second);
        assert_eq!(first.parse::<uuid::Uuid>().unwrap().get_version_num(), 4);
        assert!(validate_durable_stream_session_id(&first).is_ok());
    }

    #[test]
    fn durable_stream_session_id_url_alphabet_and_length() {
        for id in ["a", "Session_42.v1-retry", &"a".repeat(128)] {
            assert!(validate_durable_stream_session_id(id).is_ok(), "{id:?}");
        }
        for id in ["", "a/b", "a?b", "a#b", "a%b", "a b", "é", &"a".repeat(129)] {
            assert!(validate_durable_stream_session_id(id).is_err(), "{id:?}");
        }
    }
}
