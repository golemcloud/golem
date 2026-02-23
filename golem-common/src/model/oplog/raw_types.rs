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

use crate::base_model::OplogIndex;
use crate::model::Timestamp;
use crate::model::component::ComponentRevision;
use crate::model::invocation_context::{AttributeValue, InvocationContextSpan, SpanId};
use crate::model::oplog::OplogPayload;
use desert_rust::BinaryCodec;
use nonempty_collections::NEVec;
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use uuid::Uuid;

pub struct OplogIndexRange {
    current: u64,
    end: u64,
}

impl Iterator for OplogIndexRange {
    type Item = OplogIndex;

    fn next(&mut self) -> Option<Self::Item> {
        if self.current <= self.end {
            let current = self.current;
            self.current += 1; // Move forward
            Some(OplogIndex(current))
        } else {
            None
        }
    }
}

impl OplogIndexRange {
    pub fn new(start: OplogIndex, end: OplogIndex) -> OplogIndexRange {
        OplogIndexRange {
            current: start.0,
            end: end.0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AtomicOplogIndex(Arc<AtomicU64>);

impl AtomicOplogIndex {
    pub fn from_u64(value: u64) -> AtomicOplogIndex {
        AtomicOplogIndex(Arc::new(AtomicU64::new(value)))
    }

    pub fn get(&self) -> OplogIndex {
        OplogIndex(self.0.load(std::sync::atomic::Ordering::Acquire))
    }

    pub fn set(&self, value: OplogIndex) {
        self.0.store(value.0, std::sync::atomic::Ordering::Release);
    }

    pub fn from_oplog_index(value: OplogIndex) -> AtomicOplogIndex {
        AtomicOplogIndex(Arc::new(AtomicU64::new(value.0)))
    }

    /// Gets the previous oplog index
    pub fn previous(&self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
    }

    /// Gets the next oplog index
    pub fn next(&self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }

    /// Gets the last oplog index belonging to an inclusive range starting at this oplog index,
    /// having `count` elements.
    pub fn range_end(&self, count: u64) {
        self.0
            .fetch_sub(count - 1, std::sync::atomic::Ordering::AcqRel);
    }

    /// Keeps the larger value of this and `other`
    pub fn max(&self, other: OplogIndex) {
        self.0
            .fetch_max(other.0, std::sync::atomic::Ordering::AcqRel);
    }
}

impl Display for AtomicOplogIndex {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.load(std::sync::atomic::Ordering::Acquire))
    }
}

impl From<AtomicOplogIndex> for u64 {
    fn from(value: AtomicOplogIndex) -> Self {
        value.0.load(std::sync::atomic::Ordering::Acquire)
    }
}

impl From<AtomicOplogIndex> for OplogIndex {
    fn from(value: AtomicOplogIndex) -> Self {
        OplogIndex::from_u64(value.0.load(std::sync::atomic::Ordering::Acquire))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, BinaryCodec)]
#[desert(transparent)]
pub struct PayloadId(pub Uuid);

impl Default for PayloadId {
    fn default() -> Self {
        Self::new()
    }
}

impl PayloadId {
    pub fn new() -> PayloadId {
        Self(Uuid::new_v4())
    }
}

impl Display for PayloadId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Debug, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub enum SpanData {
    LocalSpan {
        span_id: SpanId,
        start: Timestamp,
        parent_id: Option<SpanId>,
        linked_context: Option<Vec<SpanData>>,
        attributes: HashMap<String, AttributeValue>,
        inherited: bool,
    },
    ExternalSpan {
        span_id: SpanId,
    },
}

impl SpanData {
    pub fn from_chain(spans: &NEVec<Arc<InvocationContextSpan>>) -> Vec<SpanData> {
        let mut result_spans = Vec::new();
        for span in spans {
            let span_data = match &**span {
                InvocationContextSpan::ExternalParent { span_id } => SpanData::ExternalSpan {
                    span_id: span_id.clone(),
                },
                InvocationContextSpan::Local {
                    span_id,
                    start,
                    state,
                    inherited,
                } => {
                    let state = state.read().unwrap();
                    let parent_id = state.parent.as_ref().map(|parent| parent.span_id().clone());
                    let linked_context = state.linked_context.as_ref().map(|linked| {
                        let linked_chain = linked.to_chain();
                        SpanData::from_chain(&linked_chain)
                    });
                    SpanData::LocalSpan {
                        span_id: span_id.clone(),
                        start: *start,
                        parent_id,
                        linked_context,
                        attributes: state.attributes.clone(),
                        inherited: *inherited,
                    }
                }
            };
            result_spans.push(span_data);
        }
        result_spans
    }
}

/// Describes a pending update
#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub enum UpdateDescription {
    /// Automatic update by replaying the oplog on the new version
    Automatic { target_revision: ComponentRevision },

    /// Custom update by loading a given snapshot on the new version
    SnapshotBased {
        target_revision: ComponentRevision,
        payload: OplogPayload<Vec<u8>>,
        mime_type: String,
    },
}

impl UpdateDescription {
    pub fn target_revision(&self) -> &ComponentRevision {
        match self {
            UpdateDescription::Automatic { target_revision } => target_revision,
            UpdateDescription::SnapshotBased {
                target_revision, ..
            } => target_revision,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub struct TimestampedUpdateDescription {
    pub timestamp: Timestamp,
    pub oplog_index: OplogIndex,
    pub description: UpdateDescription,
}

#[derive(Clone, Debug, PartialEq, Eq, BinaryCodec)]
#[desert(evolution())]
pub enum DurableFunctionType {
    /// The side-effect reads from the worker's local state (for example local file system,
    /// random generator, etc.)
    #[desert(transparent)]
    ReadLocal,
    /// The side-effect writes to the worker's local state (for example local file system)
    #[desert(transparent)]
    WriteLocal,
    /// The side-effect reads from external state (for example a key-value store)
    #[desert(transparent)]
    ReadRemote,
    /// The side-effect manipulates external state (for example an RPC call)
    #[desert(transparent)]
    WriteRemote,
    /// The side-effect manipulates external state through multiple invoked functions (for example
    /// a HTTP request where reading the response involves multiple host function calls)
    ///
    /// On the first invocation of the batch, the parameter should be `None` - this triggers
    /// writing a `BeginRemoteWrite` entry in the oplog. Followup invocations should contain
    /// this entry's index as the parameter. In batched remote writes it is the caller's responsibility
    /// to manually write an `EndRemoteWrite` entry (using `end_function`) when the operation is completed.
    #[desert(transparent)]
    WriteRemoteBatched(Option<OplogIndex>),

    #[desert(transparent)]
    WriteRemoteTransaction(Option<OplogIndex>),
}

/// Describes the error that occurred in the worker
#[derive(Clone, Debug, PartialEq, Eq, Hash, BinaryCodec)]
#[desert(evolution())]
pub enum WorkerError {
    Unknown(String),
    InvalidRequest(String),
    StackOverflow,
    OutOfMemory,
    // The worker tried to grow its memory beyond the limits of the plan
    ExceededMemoryLimit,
}

impl WorkerError {
    pub fn message(&self) -> &str {
        match self {
            Self::Unknown(message) => message,
            Self::InvalidRequest(message) => message,
            Self::StackOverflow => "Stack overflow",
            Self::OutOfMemory => "Out of memory",
            Self::ExceededMemoryLimit => "Exceeded plan memory limit",
        }
    }

    pub fn to_string(&self, error_logs: &str) -> String {
        let message = self.message();
        let error_logs = if !error_logs.is_empty() {
            format!("\n\n{error_logs}")
        } else {
            "".to_string()
        };
        format!("{message}{error_logs}")
    }
}
