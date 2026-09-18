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

use desert_rust::BinaryCodec;
use golem_schema_derive::{FromSchema, IntoSchema};

#[derive(Debug, Clone, Copy, PartialEq, Eq, BinaryCodec, IntoSchema, FromSchema)]
#[desert(evolution())]
pub enum DurableStreamMode {
    Json,
    Bytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, BinaryCodec, IntoSchema, FromSchema)]
#[desert(evolution())]
pub enum DurableStreamTransport {
    CatchUp,
    LongPoll,
    Sse,
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec, IntoSchema, FromSchema)]
#[desert(evolution())]
pub struct DurableStreamCheckpoint {
    pub offset: String,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec, IntoSchema, FromSchema)]
#[desert(evolution())]
pub struct DurableStreamReadRequest {
    pub url: String,
    pub checkpoint: DurableStreamCheckpoint,
    pub mode: DurableStreamMode,
    pub transport: DurableStreamTransport,
    pub content_type: Option<String>,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec, IntoSchema, FromSchema)]
#[desert(evolution())]
pub struct DurableStreamBatch {
    pub payload: Vec<u8>,
    pub content_type: String,
    pub next: DurableStreamCheckpoint,
    pub up_to_date: bool,
    pub closed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec, IntoSchema, FromSchema)]
#[desert(evolution())]
pub struct DurableStreamProducer {
    pub id: String,
    pub epoch: u64,
    pub sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec, IntoSchema, FromSchema)]
#[desert(evolution())]
pub enum DurableStreamAppendPayload {
    Json(Vec<String>),
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec, IntoSchema, FromSchema)]
#[desert(evolution())]
pub struct DurableStreamAppendRequest {
    pub url: String,
    pub content_type: String,
    pub payload: DurableStreamAppendPayload,
    pub producer: DurableStreamProducer,
    pub close: bool,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec, IntoSchema, FromSchema)]
#[desert(evolution())]
pub struct DurableStreamAppendReceipt {
    pub next_offset: Option<String>,
    pub epoch: u64,
    pub sequence: u64,
    pub closed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, BinaryCodec, IntoSchema, FromSchema)]
#[desert(evolution())]
pub enum DurableStreamErrorKind {
    InvalidRequest,
    PermissionDenied,
    NotFound,
    Gone,
    Closed,
    SequenceConflict,
    Fenced,
    ProducerDiverged,
    ProtocolError,
    PayloadTooLarge,
    Timeout,
    Transport,
    RateLimited,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec, IntoSchema, FromSchema)]
#[desert(evolution())]
pub struct DurableStreamError {
    pub kind: DurableStreamErrorKind,
    pub message: String,
    pub retry_after_ms: Option<u64>,
    pub producer_epoch: Option<u64>,
    pub expected_sequence: Option<u64>,
}

impl DurableStreamError {
    pub fn new(kind: DurableStreamErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            retry_after_ms: None,
            producer_epoch: None,
            expected_sequence: None,
        }
    }

    pub fn is_retryable(&self) -> bool {
        matches!(
            self.kind,
            DurableStreamErrorKind::Timeout
                | DurableStreamErrorKind::Transport
                | DurableStreamErrorKind::RateLimited
                | DurableStreamErrorKind::Unavailable
        )
    }
}
