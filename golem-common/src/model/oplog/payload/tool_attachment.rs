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

use crate::base_model::oplog::attachment::{
    ByteStreamCloseCause, ByteStreamFailure, StreamWriteError, ToolAttachmentId,
    ToolAttachmentTerminal,
};
use desert_rust::BinaryCodec;
use golem_schema_derive::{FromSchema, IntoSchema};

#[derive(Clone, Debug, Eq, PartialEq, BinaryCodec, IntoSchema, FromSchema)]
pub struct ToolAttachmentCreated {
    pub attachment: ToolAttachmentId,
    pub capacity_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, BinaryCodec, IntoSchema, FromSchema)]
pub struct ToolCollectedOutputCreated {
    pub attachment: ToolAttachmentCreated,
    pub limit_bytes: u64,
    pub admitted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, BinaryCodec, IntoSchema, FromSchema)]
pub struct ToolAttachmentWriteRequest {
    pub attachment: ToolAttachmentId,
    pub operation_ordinal: u64,
    pub length: u64,
    pub digest: Vec<u8>,
}

impl ToolAttachmentWriteRequest {
    pub fn new(attachment: ToolAttachmentId, operation_ordinal: u64, bytes: &[u8]) -> Self {
        Self {
            attachment,
            operation_ordinal,
            length: bytes.len() as u64,
            digest: blake3::hash(bytes).as_bytes().to_vec(),
        }
    }
    pub fn matches_input(&self, bytes: &[u8]) -> bool {
        self.length == bytes.len() as u64 && self.digest == blake3::hash(bytes).as_bytes()
    }
}

/// Producer outcomes contain no byte payload. Accepted bytes must be regenerated
/// and validated against the write request before restoring the local effect.
#[derive(Clone, Debug, Eq, PartialEq, BinaryCodec, IntoSchema, FromSchema)]
pub enum ToolAttachmentWriteOutcome {
    Accepted,
    Empty,
    Discarded,
    Rejected(StreamWriteError),
    HostResourceExhausted,
}

impl ToolAttachmentWriteOutcome {
    pub fn result(&self) -> Result<(), StreamWriteError> {
        match self {
            Self::Accepted | Self::Empty | Self::Discarded => Ok(()),
            Self::Rejected(error) => Err(error.clone()),
            Self::HostResourceExhausted => Err(StreamWriteError::Closed(
                ByteStreamCloseCause::Failed(ByteStreamFailure::ResourceExhausted),
            )),
        }
    }

    pub fn validate(&self, request: &ToolAttachmentWriteRequest) -> bool {
        match self {
            Self::Empty => request.length == 0 && request.matches_input(&[]),
            Self::Rejected(StreamWriteError::ConcurrentOperation) => {
                request.digest.len() == 32 && (request.length != 0 || request.matches_input(&[]))
            }
            Self::Accepted | Self::Discarded | Self::Rejected(_) | Self::HostResourceExhausted => {
                request.length != 0 && request.digest.len() == 32
            }
        }
    }

    pub fn metadata(
        &self,
        request: &ToolAttachmentWriteRequest,
        start: crate::base_model::OplogIndex,
        retained: bool,
    ) -> Option<crate::base_model::oplog::attachment::ToolAttachmentWriteMetadata> {
        if !self.validate(request) {
            return None;
        }
        Some(
            crate::base_model::oplog::attachment::ToolAttachmentWriteMetadata {
                attachment: request.attachment,
                start,
                retained,
                accepted_length: match self {
                    Self::Accepted => Some(request.length),
                    _ => None,
                },
            },
        )
    }
}

/// The selected cause is the effective, sanitized cause. A successful repeated
/// operation does not change the terminal or its origin.
#[derive(Clone, Debug, Eq, PartialEq, BinaryCodec, IntoSchema, FromSchema)]
pub enum ToolAttachmentTerminalOutcome {
    Selected(ToolAttachmentTerminal),
    Unchanged,
    Rejected(StreamWriteError),
}

impl ToolAttachmentTerminalOutcome {
    pub fn result(&self) -> Result<(), StreamWriteError> {
        match self {
            Self::Selected(_) | Self::Unchanged => Ok(()),
            Self::Rejected(error) => Err(error.clone()),
        }
    }
}
