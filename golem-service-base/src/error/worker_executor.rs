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

use desert_rust::BinaryCodec;
use golem_api_grpc::proto::golem;
use golem_common::SafeDisplay;
use golem_common::metrics::api::ApiErrorDetails;
use golem_common::model::component::{ComponentId, ComponentRevision};
use golem_common::model::oplog::WorkerError;
use golem_common::model::{PromiseId, ShardId, Timestamp, WorkerId};
use golem_wasm::wasmtime::EncodingError;
use golem_wasm_derive::{FromValue, IntoValue};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::error::Error;
use std::fmt::{Display, Formatter};
use tonic::Status;

#[derive(Debug, Clone, PartialEq, Eq, Hash, BinaryCodec)]
#[desert(evolution())]
pub enum WorkerExecutorError {
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
        reason: Box<WorkerExecutorError>,
    },
    ComponentDownloadFailed {
        component_id: ComponentId,
        component_revision: ComponentRevision,
        reason: String,
    },
    ComponentParseFailed {
        component_id: ComponentId,
        component_revision: ComponentRevision,
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
    /// The golem runtime encountered an error while exeucting the user error. Difference to ComponentTrapped is that the user component did not directly error here.
    Runtime {
        details: String,
    },
    InvalidShardId {
        shard_id: ShardId,
        shard_ids: Vec<ShardId>,
    },
    InvalidAccount,
    /// The worker failed with a TrapType::Error in a previous attempt
    PreviousInvocationFailed {
        error: WorkerError,
        stderr: String,
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
    /// The worker failed with a TrapType::Error
    InvocationFailed {
        error: WorkerError,
        stderr: String,
    },
}

impl WorkerExecutorError {
    pub fn failed_to_resume_worker(worker_id: WorkerId, reason: WorkerExecutorError) -> Self {
        Self::FailedToResumeWorker {
            worker_id,
            reason: Box::new(reason),
        }
    }

    pub fn worker_creation_failed(worker_id: WorkerId, details: impl Into<String>) -> Self {
        Self::WorkerCreationFailed {
            worker_id,
            details: details.into(),
        }
    }

    pub fn worker_not_found(worker_id: WorkerId) -> Self {
        Self::WorkerNotFound { worker_id }
    }

    pub fn worker_already_exists(worker_id: WorkerId) -> Self {
        Self::WorkerAlreadyExists { worker_id }
    }

    pub fn component_download_failed(
        component_id: ComponentId,
        component_revision: ComponentRevision,
        reason: impl Into<String>,
    ) -> Self {
        Self::ComponentDownloadFailed {
            component_id,
            component_revision,
            reason: reason.into(),
        }
    }

    pub fn initial_file_download_failed(path: String, reason: String) -> Self {
        Self::InitialComponentFileDownloadFailed { path, reason }
    }

    pub fn invalid_request(details: impl Into<String>) -> Self {
        Self::InvalidRequest {
            details: details.into(),
        }
    }

    pub fn invalid_shard_id(shard_id: ShardId, shard_ids: HashSet<ShardId>) -> Self {
        Self::InvalidShardId {
            shard_id,
            shard_ids: shard_ids.into_iter().collect(),
        }
    }

    pub fn runtime(details: impl Into<String>) -> Self {
        Self::Runtime {
            details: details.into(),
        }
    }

    pub fn unexpected_oplog_entry(expected: impl Into<String>, got: impl Into<String>) -> Self {
        Self::UnexpectedOplogEntry {
            expected: expected.into(),
            got: got.into(),
        }
    }

    pub fn unknown(details: impl Into<String>) -> Self {
        Self::Unknown {
            details: details.into(),
        }
    }
}

impl Display for WorkerExecutorError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidRequest { details } => {
                write!(f, "Invalid request: {details}")
            }
            Self::WorkerAlreadyExists { worker_id } => {
                write!(f, "Worker already exists: {worker_id}")
            }
            Self::WorkerNotFound { worker_id } => {
                write!(f, "Worker not found: {worker_id}")
            }
            Self::WorkerCreationFailed { worker_id, details } => {
                write!(f, "Failed to create worker: {worker_id}: {details}")
            }
            Self::FailedToResumeWorker { worker_id, reason } => {
                write!(f, "Failed to resume worker: {worker_id}: {reason}")
            }
            Self::ComponentDownloadFailed {
                component_id,
                component_revision,
                reason,
            } => {
                write!(
                    f,
                    "Failed to download component: {component_id}#{component_revision}: {reason}"
                )
            }
            Self::ComponentParseFailed {
                component_id,
                component_revision,
                reason,
            } => {
                write!(
                    f,
                    "Failed to parse downloaded component: {component_id}#{component_revision}: {reason}"
                )
            }
            Self::GetLatestVersionOfComponentFailed {
                component_id,
                reason,
            } => {
                write!(
                    f,
                    "Failed to get latest version of component {component_id}: {reason}"
                )
            }
            Self::InitialComponentFileDownloadFailed { path, reason } => {
                write!(
                    f,
                    "Failed to download initial file for component to {path}: {reason}"
                )
            }
            Self::PromiseNotFound { promise_id } => {
                write!(f, "Promise not found: {promise_id}")
            }
            Self::PromiseDropped { promise_id } => {
                write!(f, "Promise dropped: {promise_id}")
            }
            Self::PromiseAlreadyCompleted { promise_id } => {
                write!(f, "Promise already completed: {promise_id}")
            }
            Self::Interrupted { kind } => {
                write!(f, "{kind}")
            }
            Self::ParamTypeMismatch { details } => {
                write!(f, "Parameter type mismatch: {details}")
            }
            Self::NoValueInMessage => {
                write!(f, "No value in message")
            }
            Self::ValueMismatch { details } => {
                write!(f, "Value mismatch: {details}")
            }
            Self::UnexpectedOplogEntry { expected, got } => {
                write!(f, "Unexpected oplog entry: expected {expected}, got {got}")
            }
            Self::Runtime { details } => {
                write!(f, "Runtime error: {details}")
            }
            Self::InvalidShardId {
                shard_id,
                shard_ids,
            } => {
                write!(f, "{shard_id} is not in shards {shard_ids:?}")
            }
            Self::InvalidAccount => {
                write!(f, "Invalid account")
            }
            Self::PreviousInvocationFailed { error, stderr } => {
                write!(f, "Previous invocation failed: {}", error.to_string(stderr))
            }
            Self::PreviousInvocationExited => {
                write!(f, "The previously invoked function exited")
            }
            Self::Unknown { details } => {
                write!(f, "Unknown error: {details}")
            }
            Self::ShardingNotReady => {
                write!(f, "Sharding not ready")
            }
            Self::FileSystemError { path, reason } => {
                write!(
                    f,
                    "Failed to access file in worker filesystem {path}: {reason}"
                )
            }
            Self::InvocationFailed { error, stderr } => {
                write!(f, "Component trapped: {}", error.to_string(stderr))
            }
        }
    }
}

impl SafeDisplay for WorkerExecutorError {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

impl Error for WorkerExecutorError {
    fn description(&self) -> &str {
        match self {
            Self::InvalidRequest { .. } => "Invalid request",
            Self::WorkerAlreadyExists { .. } => "Worker already exists",
            Self::WorkerNotFound { .. } => "Worker not found",
            Self::WorkerCreationFailed { .. } => "Failed to create worker",
            Self::FailedToResumeWorker { .. } => "Failed to resume worker",
            Self::ComponentDownloadFailed { .. } => "Failed to download component",
            Self::ComponentParseFailed { .. } => "Failed to parse downloaded component",
            Self::GetLatestVersionOfComponentFailed { .. } => {
                "Failed to get latest version of component"
            }
            Self::PromiseNotFound { .. } => "Promise not found",
            Self::PromiseDropped { .. } => "Promise dropped",
            Self::PromiseAlreadyCompleted { .. } => "Promise already completed",
            Self::Interrupted { .. } => "Interrupted",
            Self::InitialComponentFileDownloadFailed { .. } => "Failed to download initial file",
            Self::ParamTypeMismatch { .. } => "Parameter type mismatch",
            Self::NoValueInMessage => "No value in message",
            Self::ValueMismatch { .. } => "Value mismatch",
            Self::UnexpectedOplogEntry { .. } => "Unexpected oplog entry",
            Self::InvalidShardId { .. } => "Invalid shard",
            Self::InvalidAccount => "Invalid account",
            Self::Runtime { .. } => "Runtime error",
            Self::InvocationFailed { .. } => "The invoked function failed",
            Self::PreviousInvocationFailed { .. } => "The previously invoked function failed",
            Self::PreviousInvocationExited => "The previously invoked function exited",
            Self::Unknown { .. } => "Unknown error",
            Self::ShardingNotReady => "Sharding not ready",
            Self::FileSystemError { .. } => "File system error",
        }
    }
}

impl ApiErrorDetails for WorkerExecutorError {
    fn trace_error_kind(&self) -> &'static str {
        match self {
            Self::InvalidRequest { .. } => "InvalidRequest",
            Self::WorkerAlreadyExists { .. } => "WorkerAlreadyExists",
            Self::WorkerNotFound { .. } => "WorkerNotFound",
            Self::WorkerCreationFailed { .. } => "WorkerCreationFailed",
            Self::FailedToResumeWorker { .. } => "FailedToResumeWorker",
            Self::ComponentDownloadFailed { .. } => "ComponentDownloadFailed",
            Self::ComponentParseFailed { .. } => "ComponentParseFailed",
            Self::GetLatestVersionOfComponentFailed { .. } => "GetLatestVersionOfComponentFailed",
            Self::InitialComponentFileDownloadFailed { .. } => "InitialComponentFileDownloadFailed",
            Self::PromiseNotFound { .. } => "PromiseNotFound",
            Self::PromiseDropped { .. } => "PromiseDropped",
            Self::PromiseAlreadyCompleted { .. } => "PromiseAlreadyCompleted",
            Self::Interrupted { .. } => "Interrupted",
            Self::ParamTypeMismatch { .. } => "ParamTypeMismatch",
            Self::NoValueInMessage => "NoValueInMessage",
            Self::ValueMismatch { .. } => "ValueMismatch",
            Self::UnexpectedOplogEntry { .. } => "UnexpectedOplogEntry",
            Self::InvalidShardId { .. } => "InvalidShardId",
            Self::InvalidAccount => "InvalidAccount",
            Self::Runtime { .. } => "Runtime",
            Self::InvocationFailed { .. } => "InvocationFailed",
            Self::PreviousInvocationFailed { .. } => "PreviousInvocationFailed",
            Self::PreviousInvocationExited => "PreviousInvocationExited",
            Self::Unknown { .. } => "Unknown",
            Self::ShardingNotReady => "ShardingNotReady",
            Self::FileSystemError { .. } => "FileSystemError",
        }
    }

    fn is_expected(&self) -> bool {
        match self {
            Self::WorkerAlreadyExists { .. }
            | Self::WorkerNotFound { .. }
            | Self::PromiseNotFound { .. }
            | Self::PromiseDropped { .. }
            | Self::PromiseAlreadyCompleted { .. }
            | Self::Interrupted { .. }
            | Self::InvalidShardId { .. } => true,
            Self::InvalidRequest { .. }
            | Self::WorkerCreationFailed { .. }
            | Self::FailedToResumeWorker { .. }
            | Self::ComponentDownloadFailed { .. }
            | Self::ComponentParseFailed { .. }
            | Self::GetLatestVersionOfComponentFailed { .. }
            | Self::InitialComponentFileDownloadFailed { .. }
            | Self::ParamTypeMismatch { .. }
            | Self::NoValueInMessage
            | Self::ValueMismatch { .. }
            | Self::UnexpectedOplogEntry { .. }
            | Self::InvalidAccount
            | Self::Runtime { .. }
            | Self::InvocationFailed { .. }
            | Self::PreviousInvocationFailed { .. }
            | Self::PreviousInvocationExited
            | Self::Unknown { .. }
            | Self::ShardingNotReady
            | Self::FileSystemError { .. } => false,
        }
    }

    fn take_cause(&mut self) -> Option<anyhow::Error> {
        None
    }
}

impl From<InterruptKind> for WorkerExecutorError {
    fn from(kind: InterruptKind) -> Self {
        Self::Interrupted { kind }
    }
}

impl From<anyhow::Error> for WorkerExecutorError {
    fn from(error: anyhow::Error) -> Self {
        match error.root_cause().downcast_ref::<InterruptKind>() {
            Some(kind) => Self::Interrupted { kind: *kind },
            None => Self::runtime(format!("{error:#?}")),
        }
    }
}

impl From<std::io::Error> for WorkerExecutorError {
    fn from(value: std::io::Error) -> Self {
        Self::Unknown {
            details: format!("{value}"),
        }
    }
}

impl From<WorkerExecutorError> for Status {
    fn from(value: WorkerExecutorError) -> Self {
        match value {
            WorkerExecutorError::InvalidRequest { details } => Self::invalid_argument(details),
            WorkerExecutorError::PromiseNotFound { promise_id } => {
                Self::not_found(format!("Promise not found: {promise_id}"))
            }
            WorkerExecutorError::WorkerNotFound { worker_id } => {
                Self::not_found(format!("Worker not found: {worker_id}"))
            }
            WorkerExecutorError::ParamTypeMismatch { details } => {
                Self::invalid_argument(format!("Parameter type mismatch: {details}"))
            }
            WorkerExecutorError::NoValueInMessage => {
                Self::invalid_argument("No value in message".to_string())
            }
            WorkerExecutorError::ValueMismatch { details } => {
                Self::invalid_argument(format!("Value mismatch: {details}"))
            }
            WorkerExecutorError::Unknown { details } => Self::unknown(details),
            WorkerExecutorError::PreviousInvocationFailed { .. } => {
                Self::failed_precondition(format!("{value}"))
            }
            _ => Self::internal(format!("{value}")),
        }
    }
}

impl From<WorkerExecutorError> for golem::worker::v1::WorkerExecutionError {
    fn from(value: WorkerExecutorError) -> Self {
        match value {
            WorkerExecutorError::InvalidRequest { details } => Self {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::InvalidRequest(
                        golem::worker::v1::InvalidRequest { details },
                    ),
                ),
            },
            WorkerExecutorError::WorkerAlreadyExists { worker_id } => Self {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::WorkerAlreadyExists(
                        golem::worker::v1::WorkerAlreadyExists {
                            worker_id: Some(worker_id.into()),
                        },
                    ),
                ),
            },
            WorkerExecutorError::WorkerNotFound { worker_id } => Self {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::WorkerNotFound(
                        golem::worker::v1::WorkerNotFound {
                            worker_id: Some(worker_id.into()),
                        },
                    ),
                ),
            },
            WorkerExecutorError::WorkerCreationFailed { worker_id, details } => Self {
                    error: Some(
                        golem::worker::v1::worker_execution_error::Error::WorkerCreationFailed(
                            golem::worker::v1::WorkerCreationFailed {
                                worker_id: Some(worker_id.into()),
                                details,
                            },
                        ),
                    ),
                },
            WorkerExecutorError::FailedToResumeWorker { worker_id, reason } =>
                Self {
                    error: Some(
                        golem::worker::v1::worker_execution_error::Error::FailedToResumeWorker(
                            Box::new(golem::worker::v1::FailedToResumeWorker {
                                worker_id: Some(worker_id.into()),
                                reason: Some(Box::new((*reason).clone().into())),
                            }),
                        ),
                    ),
                },
            WorkerExecutorError::ComponentDownloadFailed {
                component_id,
                component_revision,
                reason,
            } => Self {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::ComponentDownloadFailed(
                        golem::worker::v1::ComponentDownloadFailed {
                            component_id: Some(component_id.into()),
                            component_revision: component_revision.into(),
                            reason,
                        },
                    ),
                ),
            },
            WorkerExecutorError::ComponentParseFailed {
                component_id,
                component_revision,
                reason,
            } => Self {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::ComponentParseFailed(
                        golem::worker::v1::ComponentParseFailed {
                            component_id: Some(component_id.into()),
                            component_revision: component_revision.into(),
                            reason,
                        },
                    ),
                ),
            },
            WorkerExecutorError::GetLatestVersionOfComponentFailed {
                component_id,
                reason,
            } => Self {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::GetLatestVersionOfComponentFailed(
                        golem::worker::v1::GetLatestVersionOfComponentFailed {
                            component_id: Some(component_id.into()),
                            reason,
                        },
                    ),
                ),
            },
            WorkerExecutorError::InitialComponentFileDownloadFailed { path, reason } => Self {
                    error: Some(
                        golem::worker::v1::worker_execution_error::Error::InitialComponentFileDownloadFailed(
                            golem::worker::v1::InitialComponentFileDownloadFailed { path, reason },
                        ),
                    ),
                },
            WorkerExecutorError::PromiseNotFound { promise_id } => Self {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::PromiseNotFound(
                        golem::worker::v1::PromiseNotFound {
                            promise_id: Some(promise_id.into()),
                        },
                    ),
                ),
            },
            WorkerExecutorError::PromiseDropped { promise_id } => Self {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::PromiseDropped(
                        golem::worker::v1::PromiseDropped {
                            promise_id: Some(promise_id.into()),
                        },
                    ),
                ),
            },
            WorkerExecutorError::PromiseAlreadyCompleted { promise_id } => Self {
                    error: Some(
                        golem::worker::v1::worker_execution_error::Error::PromiseAlreadyCompleted(
                            golem::worker::v1::PromiseAlreadyCompleted {
                                promise_id: Some(promise_id.into()),
                            },
                        ),
                    ),
                },
            WorkerExecutorError::Interrupted { kind } => Self {
                error: Some(golem::worker::v1::worker_execution_error::Error::Interrupted(
                    golem::worker::v1::Interrupted {
                        recover_immediately: kind == InterruptKind::Restart,
                    },
                )),
            },
            WorkerExecutorError::ParamTypeMismatch { details } => Self {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::ParamTypeMismatch(
                        golem::worker::v1::ParamTypeMismatch { details },
                    ),
                ),
            },
            WorkerExecutorError::NoValueInMessage => Self {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::NoValueInMessage(
                        golem::worker::v1::NoValueInMessage {},
                    ),
                ),
            },
            WorkerExecutorError::ValueMismatch { details } => Self {
                error: Some(golem::worker::v1::worker_execution_error::Error::ValueMismatch(
                    golem::worker::v1::ValueMismatch { details },
                )),
            },
            WorkerExecutorError::UnexpectedOplogEntry { expected, got } => Self {
                    error: Some(
                        golem::worker::v1::worker_execution_error::Error::UnexpectedOplogEntry(
                            golem::worker::v1::UnexpectedOplogEntry { expected, got },
                        ),
                    ),
                },
            WorkerExecutorError::Runtime { details } => Self {
                error: Some(golem::worker::v1::worker_execution_error::Error::RuntimeError(
                    golem::worker::v1::RuntimeError { details },
                )),
            },
            WorkerExecutorError::InvalidShardId {
                shard_id,
                shard_ids,
            } => Self {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::InvalidShardId(
                        golem::worker::v1::InvalidShardId {
                            shard_id: Some(shard_id.into()),
                            shard_ids: shard_ids
                                .into_iter()
                                .map(|shard_id| shard_id.into())
                                .collect(),
                        },
                    ),
                ),
            },
            WorkerExecutorError::InvalidAccount => Self {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::InvalidAccount(
                        golem::worker::v1::InvalidAccount {},
                    ),
                ),
            },
            WorkerExecutorError::PreviousInvocationFailed { error, stderr } => Self {
                error: Some(golem::worker::v1::worker_execution_error::Error::PreviousInvocationFailed(
                    golem::worker::v1::PreviousInvocationFailed {
                        error: Some(error.into()),
                        stderr
                    }
                ))
            },
            WorkerExecutorError::PreviousInvocationExited => Self {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::PreviousInvocationExited(
                        golem::worker::v1::PreviousInvocationExited {},
                    ),
                ),
            },
            WorkerExecutorError::Unknown { details } => Self {
                error: Some(golem::worker::v1::worker_execution_error::Error::Unknown(
                    golem::worker::v1::UnknownError { details },
                )),
            },
            WorkerExecutorError::ShardingNotReady => Self {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::ShardingNotReady(
                        golem::worker::v1::ShardingNotReady {},
                    ),
                ),
            },
            WorkerExecutorError::FileSystemError { path, reason } => Self {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::FileSystemError(
                        golem::worker::v1::FileSystemError { path, reason },
                    ),
                ),
            },
            WorkerExecutorError::InvocationFailed { error, stderr } => Self {
                error: Some(golem::worker::v1::worker_execution_error::Error::InvocationFailed(
                    golem::worker::v1::InvocationFailed {
                        error: Some(error.into()),
                        stderr
                    }
                ))
            },
        }
    }
}

impl TryFrom<golem::worker::v1::WorkerExecutionError> for WorkerExecutorError {
    type Error = String;

    fn try_from(value: golem::worker::v1::WorkerExecutionError) -> Result<Self, Self::Error> {
        match value.error {
            None => Err("Unknown error".to_string()),
            Some(golem::worker::v1::worker_execution_error::Error::InvalidRequest(
                invalid_request,
            )) => Ok(Self::InvalidRequest {
                details: invalid_request.details,
            }),
            Some(golem::worker::v1::worker_execution_error::Error::WorkerAlreadyExists(
                worker_already_exists,
            )) => Ok(Self::WorkerAlreadyExists {
                worker_id: worker_already_exists
                    .worker_id
                    .ok_or("Missing worker_id")?
                    .try_into()?,
            }),
            Some(golem::worker::v1::worker_execution_error::Error::WorkerNotFound(
                worker_not_found,
            )) => Ok(Self::WorkerNotFound {
                worker_id: worker_not_found
                    .worker_id
                    .ok_or("Missing worker_id")?
                    .try_into()?,
            }),
            Some(golem::worker::v1::worker_execution_error::Error::WorkerCreationFailed(
                worker_creation_failed,
            )) => Ok(Self::WorkerCreationFailed {
                worker_id: worker_creation_failed
                    .worker_id
                    .ok_or("Missing worker_id")?
                    .try_into()?,
                details: worker_creation_failed.details,
            }),
            Some(golem::worker::v1::worker_execution_error::Error::FailedToResumeWorker(
                failed_to_resume_worker,
            )) => Ok(Self::FailedToResumeWorker {
                worker_id: failed_to_resume_worker
                    .worker_id
                    .ok_or("Missing worker_id")?
                    .try_into()?,
                reason: Box::new(
                    (*failed_to_resume_worker.reason.ok_or("Missing reason")?).try_into()?,
                ),
            }),
            Some(golem::worker::v1::worker_execution_error::Error::ComponentDownloadFailed(
                component_download_failed,
            )) => Ok(Self::ComponentDownloadFailed {
                component_id: component_download_failed
                    .component_id
                    .ok_or("Missing component_id")?
                    .try_into()?,
                component_revision: component_download_failed.component_revision.try_into()?,
                reason: component_download_failed.reason,
            }),
            Some(golem::worker::v1::worker_execution_error::Error::ComponentParseFailed(
                component_parse_failed,
            )) => Ok(Self::ComponentParseFailed {
                component_id: component_parse_failed
                    .component_id
                    .ok_or("Missing component_id")?
                    .try_into()?,
                component_revision: component_parse_failed.component_revision.try_into()?,
                reason: component_parse_failed.reason,
            }),
            Some(
                golem::worker::v1::worker_execution_error::Error::GetLatestVersionOfComponentFailed(
                    get_latest_version_of_component_failed,
                ),
            ) => Ok(Self::GetLatestVersionOfComponentFailed {
                component_id: get_latest_version_of_component_failed
                    .component_id
                    .ok_or("Missing component_id")?
                    .try_into()?,
                reason: get_latest_version_of_component_failed.reason,
            }),
            Some(golem::worker::v1::worker_execution_error::Error::PromiseNotFound(
                promise_not_found,
            )) => Ok(Self::PromiseNotFound {
                promise_id: promise_not_found
                    .promise_id
                    .ok_or("Missing promise_id")?
                    .try_into()?,
            }),
            Some(golem::worker::v1::worker_execution_error::Error::PromiseDropped(
                promise_dropped,
            )) => Ok(Self::PromiseDropped {
                promise_id: promise_dropped
                    .promise_id
                    .ok_or("Missing promise_id")?
                    .try_into()?,
            }),
            Some(golem::worker::v1::worker_execution_error::Error::PromiseAlreadyCompleted(
                promise_already_completed,
            )) => Ok(Self::PromiseAlreadyCompleted {
                promise_id: promise_already_completed
                    .promise_id
                    .ok_or("Missing promise_id")?
                    .try_into()?,
            }),
            Some(golem::worker::v1::worker_execution_error::Error::Interrupted(interrupted)) => {
                Ok(Self::Interrupted {
                    kind: if interrupted.recover_immediately {
                        InterruptKind::Restart
                    } else {
                        InterruptKind::Interrupt(Timestamp::now_utc())
                    },
                })
            }
            Some(golem::worker::v1::worker_execution_error::Error::ParamTypeMismatch(_)) => {
                Ok(Self::ParamTypeMismatch {
                    details: "".to_string(),
                })
            }
            Some(golem::worker::v1::worker_execution_error::Error::NoValueInMessage(_)) => {
                Ok(Self::NoValueInMessage)
            }
            Some(golem::worker::v1::worker_execution_error::Error::ValueMismatch(
                value_mismatch,
            )) => Ok(Self::ValueMismatch {
                details: value_mismatch.details,
            }),
            Some(golem::worker::v1::worker_execution_error::Error::UnexpectedOplogEntry(
                unexpected_oplog_entry,
            )) => Ok(Self::UnexpectedOplogEntry {
                expected: unexpected_oplog_entry.expected,
                got: unexpected_oplog_entry.got,
            }),
            Some(golem::worker::v1::worker_execution_error::Error::InvalidShardId(
                invalid_shard_id,
            )) => Ok(Self::InvalidShardId {
                shard_id: invalid_shard_id.shard_id.ok_or("Missing shard_id")?.into(),
                shard_ids: invalid_shard_id
                    .shard_ids
                    .into_iter()
                    .map(|id| id.into())
                    .collect(),
            }),
            Some(golem::worker::v1::worker_execution_error::Error::InvalidAccount(_)) => {
                Ok(Self::InvalidAccount)
            }
            Some(golem::worker::v1::worker_execution_error::Error::RuntimeError(runtime_error)) => {
                Ok(Self::Runtime {
                    details: runtime_error.details,
                })
            }
            Some(golem::worker::v1::worker_execution_error::Error::PreviousInvocationFailed(
                inner,
            )) => Ok(Self::PreviousInvocationFailed {
                error: inner.error.ok_or("no trap_cause field")?.try_into()?,
                stderr: inner.stderr
             }),
            Some(golem::worker::v1::worker_execution_error::Error::PreviousInvocationExited(_)) => {
                Ok(Self::PreviousInvocationExited)
            }
            Some(golem::worker::v1::worker_execution_error::Error::Unknown(unknown_error)) => {
                Ok(Self::Unknown {
                    details: unknown_error.details,
                })
            }
            Some(golem::worker::v1::worker_execution_error::Error::ShardingNotReady(_)) => {
                Ok(Self::ShardingNotReady)
            }
            Some(golem::worker::v1::worker_execution_error::Error::InitialComponentFileDownloadFailed(
                initial_file_download_failed,
            )) => Ok(Self::InitialComponentFileDownloadFailed {
                path: initial_file_download_failed.path,
                reason: initial_file_download_failed.reason,
            }),
            Some(golem::worker::v1::worker_execution_error::Error::FileSystemError(
                file_system_error,
            )) => Ok(Self::FileSystemError {
                path: file_system_error.path,
                reason: file_system_error.reason,
            }),
            Some(golem::worker::v1::worker_execution_error::Error::InvocationFailed(
                inner,
            )) => Ok(Self::InvocationFailed {
                error: inner.error.ok_or("no trap_cause field")?.try_into()?,
                stderr: inner.stderr
             }),
        }
    }
}

impl From<EncodingError> for WorkerExecutorError {
    fn from(value: EncodingError) -> Self {
        match value {
            EncodingError::ParamTypeMismatch { details } => Self::ParamTypeMismatch { details },
            EncodingError::ValueMismatch { details } => Self::ValueMismatch { details },
            EncodingError::Unknown { details } => Self::Unknown { details },
        }
    }
}

#[derive(Debug, Clone, PartialOrd, PartialEq, Eq, Hash)]
pub enum GolemSpecificWasmTrap {
    WorkerOutOfMemory,
    WorkerExceededMemoryLimit,
}

impl Display for GolemSpecificWasmTrap {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WorkerOutOfMemory => write!(f, "Worker cannot acquire more memory"),
            Self::WorkerExceededMemoryLimit => write!(f, "Worker exceeded plan memory limits"),
        }
    }
}

impl Error for GolemSpecificWasmTrap {}

#[derive(
    Debug,
    Copy,
    Clone,
    PartialOrd,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    BinaryCodec,
    IntoValue,
    FromValue,
)]
pub enum InterruptKind {
    Interrupt(Timestamp),
    Restart,
    Suspend(Timestamp),
    Jump,
}

impl Display for InterruptKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            InterruptKind::Interrupt(_) => write!(f, "Interrupted via the Golem API"),
            InterruptKind::Restart => write!(f, "Simulated crash via the Golem API"),
            InterruptKind::Suspend(_) => write!(f, "Suspended"),
            InterruptKind::Jump => write!(f, "Jumping back in time"),
        }
    }
}

impl Error for InterruptKind {}

#[cfg(feature = "worker-executor")]
mod service {
    use super::WorkerExecutorError;

    impl From<WorkerExecutorError> for wasmtime_wasi::p2::StreamError {
        fn from(value: WorkerExecutorError) -> Self {
            Self::Trap(wasmtime::Error::msg(value.to_string()))
        }
    }

    impl From<WorkerExecutorError> for wasmtime_wasi::p2::SocketError {
        fn from(value: WorkerExecutorError) -> Self {
            Self::trap(value)
        }
    }
}
