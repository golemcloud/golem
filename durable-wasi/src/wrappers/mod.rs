// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use bincode::{Decode, Encode};
use golem_common::base_model::{ComponentId, PromiseId, ShardId, WorkerId};
use std::fmt::{Display, Formatter};
use crate::bindings;
use crate::bindings::exports::wasi::clocks::wall_clock;

mod clock;

// TODO: try to avoid having copies of these types here
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub enum RpcError {
    ProtocolError { details: String },
    Denied { details: String },
    NotFound { details: String },
    RemoteInternalError { details: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub enum WorkerProxyError {
    BadRequest(Vec<String>),
    Unauthorized(String),
    LimitExceeded(String),
    NotFound(String),
    AlreadyExists(String),
    InternalError(GolemError),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub enum GolemError {
    InvalidRequest {
        details: String,
    },
    WorkerAlreadyExists {
        worker_id: WorkerId,
    },
    WorkerNotFound {
        worker_id: WorkerId,
    },
    WorkerCreationFailed {
        worker_id: WorkerId,
        details: String,
    },
    FailedToResumeWorker {
        worker_id: WorkerId,
        reason: Box<GolemError>,
    },
    ComponentDownloadFailed {
        component_id: ComponentId,
        component_version: u64,
        reason: String,
    },
    ComponentParseFailed {
        component_id: ComponentId,
        component_version: u64,
        reason: String,
    },
    GetLatestVersionOfComponentFailed {
        component_id: ComponentId,
        reason: String,
    },
    PromiseNotFound {
        promise_id: PromiseId,
    },
    PromiseDropped {
        promise_id: PromiseId,
    },
    PromiseAlreadyCompleted {
        promise_id: PromiseId,
    },
    Interrupted {
        kind: InterruptKind,
    },
    ParamTypeMismatch {
        details: String,
    },
    NoValueInMessage,
    ValueMismatch {
        details: String,
    },
    UnexpectedOplogEntry {
        expected: String,
        got: String,
    },
    Runtime {
        details: String,
    },
    InvalidShardId {
        shard_id: ShardId,
        shard_ids: Vec<ShardId>,
    },
    InvalidAccount,
    PreviousInvocationFailed {
        details: String,
    },
    PreviousInvocationExited,
    Unknown {
        details: String,
    },
    ShardingNotReady,
    InitialComponentFileDownloadFailed {
        path: String,
        reason: String,
    },
    FileSystemError {
        path: String,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialOrd, PartialEq, Eq, Hash, Encode, Decode)]
pub enum InterruptKind {
    Interrupt,
    Restart,
    Suspend,
    Jump,
}

// Guest binding version of golem-worker-executor-base's SerializableError
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub enum SerializableError {
    Generic { message: String },
    FsError { code: u8 },
    Golem { error: GolemError },
    SocketError { code: u8 },
    Rpc { error: RpcError },
    WorkerProxy { error: WorkerProxyError },
}

// TODO: real implementation
impl Display for SerializableError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            SerializableError::Generic { message } => write!(f, "{}", message),
            SerializableError::FsError { code } => write!(f, "FsError({})", code),
            SerializableError::Golem { error } => write!(f, "Golem({:?})", error),
            SerializableError::SocketError { code } => write!(f, "SocketError({})", code),
            SerializableError::Rpc { error } => write!(f, "Rpc({:?})", error),
            SerializableError::WorkerProxy { error } => write!(f, "WorkerProxy({:?})", error),
        }
    }
}


#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub struct SerializableDateTime {
    pub seconds: u64,
    pub nanoseconds: u32,
}

impl From<wall_clock::Datetime> for SerializableDateTime {
    fn from(value: wall_clock::Datetime) -> Self {
        Self {
            seconds: value.seconds,
            nanoseconds: value.nanoseconds,
        }
    }
}

impl From<SerializableDateTime> for wall_clock::Datetime {
    fn from(value: SerializableDateTime) -> Self {
        Self {
            seconds: value.seconds,
            nanoseconds: value.nanoseconds,
        }
    }
}


impl From<bindings::wasi::clocks::wall_clock::Datetime> for SerializableDateTime {
    fn from(value: bindings::wasi::clocks::wall_clock::Datetime) -> Self {
        Self {
            seconds: value.seconds,
            nanoseconds: value.nanoseconds,
        }
    }
}

impl From<SerializableDateTime> for bindings::wasi::clocks::wall_clock::Datetime {
    fn from(value: SerializableDateTime) -> Self {
        Self {
            seconds: value.seconds,
            nanoseconds: value.nanoseconds,
        }
    }
}

