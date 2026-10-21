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

use std::collections::{BTreeMap, HashMap};
use std::fmt::Debug;
use std::ops::Bound;

use bigdecimal::BigDecimal;
use bit_vec::BitVec;
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use desert_rust::BinaryCodec;
use golem_schema::schema::wit::wire::ValueNodeIndex as NodeIndex;
use mac_address::MacAddress;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use wasmtime_wasi::p2::bindings::sockets::network::ErrorCode as SocketErrorCode;

use crate::base_model::TransactionId;
use crate::base_model::durable_stream::{
    StreamBindingRecord, StreamConsumerObservation, StreamRegistrationInvocation,
};
use crate::base_model::oplog::attachment::{ToolAttachmentId, ToolAttachmentTerminal};
pub use crate::base_model::tool::{
    SerializableCustomToolError, SerializableToolError, SerializableToolInvocationResult,
    SerializableToolRpcError,
};
use crate::model::component::ComponentRevision;
use crate::model::environment::EnvironmentId;
use crate::model::{AgentId, AgentStatus, RdbmsPoolKey};
use crate::schema::TypedSchemaValue;
use crate::schema::schema_value::SchemaValue;
use crate::schema::tool::DiscoveredTool;

#[derive(
    Debug,
    Clone,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub struct SerializableToolDiscoverySnapshot {
    pub deployment_revision: Option<u64>,
    pub dynamic_tools: Vec<SerializableMcpImportDiscovery>,
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
pub struct SerializableMcpImportDiscovery {
    pub import_index: u32,
    pub tools: SerializableDiscoveredTools,
    pub exclusions: Vec<(String, String)>,
}

/// Tool definitions remain structured in the durable payload, but use JSON text in the
/// public schema representation so metadata type trees cannot exhaust Protobuf's depth limit.
#[derive(Debug, Clone, PartialEq, Default, BinaryCodec)]
#[desert(transparent)]
pub struct SerializableDiscoveredTools(pub Vec<DiscoveredTool>);

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
pub struct SecretRevealAudit {
    pub calling_agent: AgentId,
    pub config_key: Option<Vec<String>>,
    pub timestamp: SerializableDateTime,
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
pub enum SecretRevealError {
    Unavailable(String),
    VersionNotFound(u64),
    Internal(String),
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
pub enum PermissionCardRevokeError {
    AlreadyRevoked(String),
    CardRevoked(String),
    NotPermitted(String),
}

/// A typed refusal of a runtime permission-card creation, returned to the guest instead of the
/// created card or the completed install.
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
pub enum PermissionCardDeriveError {
    CardRevoked(String),
    CardExpired(String),
    NotPermitted(String),
}

/// A `golem:permissions` permission error that a recorded authorization decision returns to the
/// guest.
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
pub enum SerializablePermissionError {
    NotPermitted(String),
    LowerBoundTooBroad(String),
    UpperBoundTooBroad(String),
    WalletMultiSourceRequired(String),
    CardRevoked(String),
    CardExpired(String),
    InstallRecipientMismatch(String),
    UnknownResourceClass(String),
    InvalidPattern(String),
    InvalidVerb(String),
    InvalidOwner(String),
    InvalidRecipient(String),
}

/// The liveness of the card behind a permission-card handle when an operation used the handle.
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
pub enum SerializablePermissionCardLiveness {
    /// The card is live. The payload is the serialized `StoredCard` that the handle resolves to.
    Live(Vec<u8>),
    /// The card is revoked, or the registry does not know it.
    Revoked,
    /// The card expired.
    Expired,
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
/// A point in time relative to the Unix epoch.
///
/// `seconds` is the *floored* number of whole seconds since the epoch (negative
/// for pre-epoch instants) and `nanoseconds` is the offset within that second,
/// always in `0..1_000_000_000`. This matches the WASI P3 `system-clock`
/// `instant` representation, so P3 conversions are lossless field copies.
pub struct SerializableDateTime {
    pub seconds: i64,
    pub nanoseconds: u32,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerializableFsErrorCode(wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode);

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SerializableSocketErrorCode(SocketErrorCode);

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

/// Serializable form of a p3 `http::client::send` request head — the Start
/// payload of the `P3HttpClientSend` oplog pair.
///
/// Records everything needed to identify and replay the outgoing request
/// except the body bytes. The outgoing request body is a `stream<u8>` the guest
/// writes; its bytes are owned by that stream's own durable wrapper (the
/// outgoing-body stream path), so they are intentionally not duplicated here.
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
    HttpError(SerializableHttpErrorCode),
    /// The outgoing request body is durably recorded as `HostStreamFrame` hint entries under the
    /// send's `Start` (see [`SerializableP3HttpRequestBodyFrame`]), so an interrupted response-body
    /// stream can be rebuilt after a restart by re-issuing the request with the recorded body.
    SuccessWithRecordedRequestBody {
        headers: SerializableResponseHeaders,
        /// Whether the recording had already reached its terminal frame when
        /// the send's `End` was appended. When `false`, the request body was
        /// still streaming at that point, so replay must treat the recording
        /// as potentially truncated and be prepared to resume it from the
        /// replayed guest body.
        recording_complete_at_end: bool,
    },
}

/// Terminal of a p3 response `consume-body` — the End payload of the
/// `P3HttpClientConsumeBody` oplog pair.
///
/// The response body *bytes* are not recorded here; they are persisted
/// chunk-by-chunk as [`SerializableP3HttpBodyChunk`] child calls under the
/// `consume-body` batched scope and replayed lazily. This enum captures only
/// how the body stream terminated:
///
/// * `Trailers(None)` — the body closed cleanly with no trailers (also used
///   when the guest dropped the stream before it completed).
/// * `Trailers(Some(..))` — the body closed cleanly and delivered trailers.
/// * `HttpError(..)` — the body errored before completing. In p3 a body error
///   is surfaced to the guest via the trailers future's `ErrorCode`, not via
///   the body stream, so it round-trips here rather than on the stream.
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

/// One persisted unit of a P3 HTTP response body stream.
///
/// The body is recorded chunk-by-chunk as child durable calls under the
/// `consume-body` batched scope (mirroring the per-read durability of the P2
/// incoming body stream): every non-empty upstream frame becomes a `Data`
/// chunk, persisted before its bytes are delivered to the guest, and a single
/// `End` chunk terminates the recorded stream so replay knows when to stop
/// reading children. The trailers / body-error terminal is carried by the
/// parent `consume-body` call's [`SerializableP3HttpConsumeBodyResult`].
#[derive(
    Debug,
    Clone,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub enum SerializableP3HttpBodyChunk {
    Data(Vec<u8>),
    End,
    Cancelled,
}

/// One persisted unit of a P3 HTTP *outgoing request* body stream.
///
/// The request body produced by the guest is recorded frame-by-frame as
/// `HostStreamFrame` *hint* oplog entries referencing the send's host-call
/// `Start` (the outgoing mirror of [`SerializableP3HttpBodyChunk`], which uses
/// child durable calls instead): every guest-produced data frame becomes a
/// `Data` frame and trailers become a `Trailers` frame, each persisted before
/// the frame is released to the wire. A single terminal frame — `End` for a
/// cleanly finished body or `Error` for a guest-side body failure — closes the
/// recorded stream. Because the entries are hints, the replay cursor skips
/// them; consumers find them by scanning the oplog for `HostStreamFrame`
/// entries whose `parent_start_index` matches the send's `Start`. The recorded
/// frames are what in-function retry and post-restart rebuild replay to
/// re-issue the request with a byte-identical body, streaming from the oplog
/// so the body is never buffered whole in memory.
///
/// `Data` frames carry the byte `offset` of the frame's first byte within the
/// logical request-body byte stream. A send whose recording was left truncated
/// by a crash resumes recording under the same `Start` index after replay, so
/// a second recorded sequence can follow the first run's truncated one. The
/// guest's body production is deterministic, so every such sequence is a
/// prefix of the same byte stream: consumers merge sequences by offset (a
/// frame restarting at an already-covered offset re-covers known bytes)
/// instead of concatenating them.
#[derive(
    Debug,
    Clone,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub enum SerializableP3HttpRequestBodyFrame {
    Data { offset: u64, bytes: Vec<u8> },
    Trailers(Option<HashMap<String, Vec<Vec<u8>>>>),
    End,
    Error(SerializableHttpErrorCode),
}

/// One persisted unit of a P3 TCP socket receive stream.
///
/// Incoming socket bytes are recorded chunk-by-chunk as child durable calls
/// under the `receive` batched scope (mirroring the chunk-by-chunk durability
/// of the P3 HTTP response body stream): every non-empty upstream chunk becomes
/// a `Data` chunk, persisted before its bytes are delivered to the guest, and a
/// single `End` chunk terminates the recorded stream so replay knows when to
/// stop reading children. The terminal `Result<(), error-code>` is carried by
/// the parent `receive` call's response payload.
#[derive(
    Debug,
    Clone,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub enum SerializableP3TcpChunk {
    Data(Vec<u8>),
    End,
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

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub enum SerializableIpAddress {
    IPv4 { address: [u8; 4] },
    IPv6 { address: [u16; 8] },
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
    pub address: SerializableIpAddress,
    pub port: u16,
    pub flow_info: Option<u32>,
    pub scope_id: Option<u32>,
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
    FailedClassified {
        kind: SerializableHostFailureKind,
        message: String,
    },
    Pending,
    Completed(Result<SchemaValue, SerializableRpcError>),
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub enum SerializableHostFailureKind {
    Transient,
    Permanent,
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
pub enum SerializableRpcError {
    ProtocolError {
        details: String,
    },
    Denied {
        details: String,
    },
    NotFound {
        details: String,
    },
    RemoteInternalError {
        details: String,
    },
    RemoteAgentError {
        error: Box<crate::model::agent::AgentError>,
    },
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub enum SerializableEntityBodyExecution {
    Executed,
    Skipped,
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
pub struct SerializableToolResultValue {
    value: TypedSchemaValue,
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
pub struct SerializableToolStructuredResult {
    pub result: Option<SerializableToolResultValue>,
}

/// Schema-native terminal of the generic tool entity invocation. Stdout is deliberately absent:
/// attachment events and their terminal are owned by the live operation rather than the oplog
/// response.
#[derive(
    Debug,
    Clone,
    PartialEq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub struct SerializableToolOperationTerminal {
    pub body_execution: SerializableEntityBodyExecution,
    pub result: Result<SerializableToolStructuredResult, SerializableToolRpcError>,
    pub admission_refusal: Option<SerializableToolAdmissionRefusal>,
}

/// The admission decision that interrupted an executing tool body. This is independent of
/// the body's result: a host adapter may deliver the refusal as a catchable error.
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
pub enum SerializableToolAdmissionRefusal {
    BeforeStart {
        initiating_function: String,
        /// Encoded claim descriptor, including its invocation ID and ownership. Replay
        /// compares the decoded descriptor rather than serialized byte ordering.
        claim: Vec<u8>,
    },
    AtRecordedOplogEntry {
        index: u64,
    },
    BeforePositional {
        request: SerializableToolPositionalRequest,
    },
    BeforeInputRead {
        session: StreamRegistrationInvocation,
        binding: Box<StreamBindingRecord>,
        consumer_read_ordinal: u64,
        observation: StreamConsumerObservation,
    },
    BeforeAttachmentDisposal {
        attachment: ToolAttachmentId,
        terminal: ToolAttachmentTerminal,
    },
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
pub enum SerializableToolPositionalRequest {
    SetRetryPolicy,
    RemoveRetryPolicy,
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
    Copy,
    PartialEq,
    Eq,
    BinaryCodec,
    golem_schema_derive::IntoSchema,
    golem_schema_derive::FromSchema,
)]
#[desert(evolution())]
pub enum SerializableWebsocketReconstructionPolicy {
    ReconnectAutomatically,
    ReportConnectionLoss,
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
    SessionLost,
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
    ResourceLost(String),
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
pub enum SerializableRdbmsTransactionTerminal {
    Commit(Result<(), SerializableRdbmsError>),
    Rollback(Result<(), SerializableRdbmsError>),
    Drop(Result<(), SerializableRdbmsError>),
    BeginFailed(Result<(), SerializableRdbmsError>),
}

#[derive(Clone, Debug, PartialEq, BinaryCodec)]
#[desert(transparent)]
pub struct SerializableMacAddress(pub MacAddress);

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

pub use spans::{decode_span_data, encode_span_data};

mod tools;

mod time;

mod filesystem;

mod sockets;

mod http;

mod agent;

mod spans;

mod rdbms;

#[cfg(test)]
mod tests;
