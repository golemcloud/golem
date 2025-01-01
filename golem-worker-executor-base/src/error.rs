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

use std::collections::HashSet;
use std::error::Error;
use std::fmt::{Display, Formatter};

use bincode::{Decode, Encode};
use golem_api_grpc::proto::golem;
use golem_common::metrics::api::TraceErrorKind;
use golem_common::model::{ComponentId, PromiseId, ShardId, WorkerId};
use golem_wasm_rpc::wasmtime::EncodingError;
use tonic::Status;

use crate::model::InterruptKind;

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

impl GolemError {
    pub fn failed_to_resume_worker(worker_id: WorkerId, reason: GolemError) -> Self {
        GolemError::FailedToResumeWorker {
            worker_id,
            reason: Box::new(reason),
        }
    }

    pub fn worker_creation_failed(worker_id: WorkerId, details: impl Into<String>) -> Self {
        GolemError::WorkerCreationFailed {
            worker_id,
            details: details.into(),
        }
    }

    pub fn worker_not_found(worker_id: WorkerId) -> Self {
        GolemError::WorkerNotFound { worker_id }
    }

    pub fn worker_already_exists(worker_id: WorkerId) -> Self {
        GolemError::WorkerAlreadyExists { worker_id }
    }

    pub fn component_download_failed(
        component_id: ComponentId,
        component_version: u64,
        reason: impl Into<String>,
    ) -> Self {
        GolemError::ComponentDownloadFailed {
            component_id,
            component_version,
            reason: reason.into(),
        }
    }

    pub fn initial_file_download_failed(path: String, reason: String) -> Self {
        GolemError::InitialComponentFileDownloadFailed { path, reason }
    }

    pub fn invalid_request(details: impl Into<String>) -> Self {
        GolemError::InvalidRequest {
            details: details.into(),
        }
    }

    pub fn invalid_shard_id(shard_id: ShardId, shard_ids: HashSet<ShardId>) -> Self {
        GolemError::InvalidShardId {
            shard_id,
            shard_ids: shard_ids.into_iter().collect(),
        }
    }

    pub fn runtime(details: impl Into<String>) -> Self {
        GolemError::Runtime {
            details: details.into(),
        }
    }

    pub fn unexpected_oplog_entry(expected: impl Into<String>, got: impl Into<String>) -> Self {
        GolemError::UnexpectedOplogEntry {
            expected: expected.into(),
            got: got.into(),
        }
    }

    pub fn unknown(details: impl Into<String>) -> Self {
        GolemError::Unknown {
            details: details.into(),
        }
    }
}

impl Display for GolemError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            GolemError::InvalidRequest { details } => {
                write!(f, "Invalid request: {details}")
            }
            GolemError::WorkerAlreadyExists { worker_id } => {
                write!(f, "Worker already exists: {worker_id}")
            }
            GolemError::WorkerNotFound { worker_id } => {
                write!(f, "Worker not found: {worker_id}")
            }
            GolemError::WorkerCreationFailed { worker_id, details } => {
                write!(f, "Failed to create worker: {worker_id}: {details}")
            }
            GolemError::FailedToResumeWorker { worker_id, reason } => {
                write!(f, "Failed to resume worker: {worker_id}: {reason}")
            }
            GolemError::ComponentDownloadFailed {
                component_id,
                component_version,
                reason,
            } => {
                write!(
                    f,
                    "Failed to download component: {component_id}#{component_version}: {reason}"
                )
            }
            GolemError::ComponentParseFailed {
                component_id,
                component_version,
                reason,
            } => {
                write!(
                    f,
                    "Failed to parse downloaded component: {component_id}#{component_version}: {reason}"
                )
            }
            GolemError::GetLatestVersionOfComponentFailed {
                component_id,
                reason,
            } => {
                write!(
                    f,
                    "Failed to get latest version of component {component_id}: {reason}"
                )
            }
            GolemError::InitialComponentFileDownloadFailed { path, reason } => {
                write!(
                    f,
                    "Failed to download initial file for component to {path}: {reason}"
                )
            }
            GolemError::PromiseNotFound { promise_id } => {
                write!(f, "Promise not found: {promise_id}")
            }
            GolemError::PromiseDropped { promise_id } => {
                write!(f, "Promise dropped: {promise_id}")
            }
            GolemError::PromiseAlreadyCompleted { promise_id } => {
                write!(f, "Promise already completed: {promise_id}")
            }
            GolemError::Interrupted { kind } => {
                write!(f, "{kind}")
            }
            GolemError::ParamTypeMismatch { details } => {
                write!(f, "Parameter type mismatch: {details}")
            }
            GolemError::NoValueInMessage => {
                write!(f, "No value in message")
            }
            GolemError::ValueMismatch { details } => {
                write!(f, "Value mismatch: {details}")
            }
            GolemError::UnexpectedOplogEntry { expected, got } => {
                write!(f, "Unexpected oplog entry: expected {expected}, got {got}")
            }
            GolemError::Runtime { details } => {
                write!(f, "Runtime error: {details}")
            }
            GolemError::InvalidShardId {
                shard_id,
                shard_ids,
            } => {
                write!(f, "{} is not in shards {:?}", shard_id, shard_ids)
            }
            GolemError::InvalidAccount => {
                write!(f, "Invalid account")
            }
            GolemError::PreviousInvocationFailed { details } => {
                write!(f, "The previously invoked function failed: {details}")
            }
            GolemError::PreviousInvocationExited => {
                write!(f, "The previously invoked function exited")
            }
            GolemError::Unknown { details } => {
                write!(f, "Unknown error: {details}")
            }
            GolemError::ShardingNotReady => {
                write!(f, "Sharding not ready")
            }
            GolemError::FileSystemError { path, reason } => {
                write!(
                    f,
                    "Failed to access file in worker filesystem {path}: {reason}"
                )
            }
        }
    }
}

impl Error for GolemError {
    fn description(&self) -> &str {
        match self {
            GolemError::InvalidRequest { .. } => "Invalid request",
            GolemError::WorkerAlreadyExists { .. } => "Worker already exists",
            GolemError::WorkerNotFound { .. } => "Worker not found",
            GolemError::WorkerCreationFailed { .. } => "Failed to create worker",
            GolemError::FailedToResumeWorker { .. } => "Failed to resume worker",
            GolemError::ComponentDownloadFailed { .. } => "Failed to download component",
            GolemError::ComponentParseFailed { .. } => "Failed to parse downloaded component",
            GolemError::GetLatestVersionOfComponentFailed { .. } => {
                "Failed to get latest version of component"
            }
            GolemError::PromiseNotFound { .. } => "Promise not found",
            GolemError::PromiseDropped { .. } => "Promise dropped",
            GolemError::PromiseAlreadyCompleted { .. } => "Promise already completed",
            GolemError::Interrupted { .. } => "Interrupted",
            GolemError::InitialComponentFileDownloadFailed { .. } => {
                "Failed to download initial file"
            }
            GolemError::ParamTypeMismatch { .. } => "Parameter type mismatch",
            GolemError::NoValueInMessage => "No value in message",
            GolemError::ValueMismatch { .. } => "Value mismatch",
            GolemError::UnexpectedOplogEntry { .. } => "Unexpected oplog entry",
            GolemError::InvalidShardId { .. } => "Invalid shard",
            GolemError::InvalidAccount => "Invalid account",
            GolemError::Runtime { .. } => "Runtime error",
            GolemError::PreviousInvocationFailed { .. } => "The previously invoked function failed",
            GolemError::PreviousInvocationExited => "The previously invoked function exited",
            GolemError::Unknown { .. } => "Unknown error",
            GolemError::ShardingNotReady => "Sharding not ready",
            GolemError::FileSystemError { .. } => "File system error",
        }
    }
}

impl TraceErrorKind for GolemError {
    fn trace_error_kind(&self) -> &'static str {
        match self {
            GolemError::InvalidRequest { .. } => "InvalidRequest",
            GolemError::WorkerAlreadyExists { .. } => "WorkerAlreadyExists",
            GolemError::WorkerNotFound { .. } => "WorkerNotFound",
            GolemError::WorkerCreationFailed { .. } => "WorkerCreationFailed",
            GolemError::FailedToResumeWorker { .. } => "FailedToResumeWorker",
            GolemError::ComponentDownloadFailed { .. } => "ComponentDownloadFailed",
            GolemError::ComponentParseFailed { .. } => "ComponentParseFailed",
            GolemError::GetLatestVersionOfComponentFailed { .. } => {
                "GetLatestVersionOfComponentFailed"
            }
            GolemError::InitialComponentFileDownloadFailed { .. } => {
                "InitialComponentFileDownloadFailed"
            }
            GolemError::PromiseNotFound { .. } => "PromiseNotFound",
            GolemError::PromiseDropped { .. } => "PromiseDropped",
            GolemError::PromiseAlreadyCompleted { .. } => "PromiseAlreadyCompleted",
            GolemError::Interrupted { .. } => "Interrupted",
            GolemError::ParamTypeMismatch { .. } => "ParamTypeMismatch",
            GolemError::NoValueInMessage => "NoValueInMessage",
            GolemError::ValueMismatch { .. } => "ValueMismatch",
            GolemError::UnexpectedOplogEntry { .. } => "UnexpectedOplogEntry",
            GolemError::InvalidShardId { .. } => "InvalidShardId",
            GolemError::InvalidAccount => "InvalidAccount",
            GolemError::Runtime { .. } => "Runtime",
            GolemError::PreviousInvocationFailed { .. } => "PreviousInvocationFailed",
            GolemError::PreviousInvocationExited => "PreviousInvocationExited",
            GolemError::Unknown { .. } => "Unknown",
            GolemError::ShardingNotReady => "ShardingNotReady",
            GolemError::FileSystemError { .. } => "FileSystemError",
        }
    }
}

impl From<InterruptKind> for GolemError {
    fn from(kind: InterruptKind) -> Self {
        GolemError::Interrupted { kind }
    }
}

impl From<anyhow::Error> for GolemError {
    fn from(error: anyhow::Error) -> Self {
        match error.root_cause().downcast_ref::<InterruptKind>() {
            Some(kind) => GolemError::Interrupted { kind: kind.clone() },
            None => GolemError::runtime(format!("{error:#?}")),
        }
    }
}

impl From<GolemError> for Status {
    fn from(value: GolemError) -> Self {
        match value {
            GolemError::InvalidRequest { details } => Status::invalid_argument(details),
            GolemError::PromiseNotFound { promise_id } => Status::not_found(format!(
                "Promise not found: {promise_id}",
                promise_id = promise_id
            )),
            GolemError::WorkerNotFound { worker_id } => Status::not_found(format!(
                "Worker not found: {worker_id}",
                worker_id = worker_id
            )),
            GolemError::ParamTypeMismatch { details } => {
                Status::invalid_argument(format!("Parameter type mismatch: {details}"))
            }
            GolemError::NoValueInMessage => {
                Status::invalid_argument("No value in message".to_string())
            }
            GolemError::ValueMismatch { details } => {
                Status::invalid_argument(format!("Value mismatch: {details}"))
            }
            GolemError::Unknown { details } => Status::unknown(details),
            _ => Status::internal(format!("{value}")),
        }
    }
}

impl From<GolemError> for golem::worker::v1::WorkerExecutionError {
    fn from(value: GolemError) -> Self {
        match value {
            GolemError::InvalidRequest { details } => golem::worker::v1::WorkerExecutionError {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::InvalidRequest(
                        golem::worker::v1::InvalidRequest { details },
                    ),
                ),
            },
            GolemError::WorkerAlreadyExists { worker_id } => golem::worker::v1::WorkerExecutionError {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::WorkerAlreadyExists(
                        golem::worker::v1::WorkerAlreadyExists {
                            worker_id: Some(worker_id.into()),
                        },
                    ),
                ),
            },
            GolemError::WorkerNotFound { worker_id } => golem::worker::v1::WorkerExecutionError {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::WorkerNotFound(
                        golem::worker::v1::WorkerNotFound {
                            worker_id: Some(worker_id.into()),
                        },
                    ),
                ),
            },
            GolemError::WorkerCreationFailed { worker_id, details } => {
                golem::worker::v1::WorkerExecutionError {
                    error: Some(
                        golem::worker::v1::worker_execution_error::Error::WorkerCreationFailed(
                            golem::worker::v1::WorkerCreationFailed {
                                worker_id: Some(worker_id.into()),
                                details,
                            },
                        ),
                    ),
                }
            }
            GolemError::FailedToResumeWorker { worker_id, reason } => {
                golem::worker::v1::WorkerExecutionError {
                    error: Some(
                        golem::worker::v1::worker_execution_error::Error::FailedToResumeWorker(
                            Box::new(golem::worker::v1::FailedToResumeWorker {
                                worker_id: Some(worker_id.into()),
                                reason: Some(Box::new((*reason).clone().into())),
                            }),
                        ),
                    ),
                }
            }
            GolemError::ComponentDownloadFailed {
                component_id,
                component_version,
                reason,
            } => golem::worker::v1::WorkerExecutionError {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::ComponentDownloadFailed(
                        golem::worker::v1::ComponentDownloadFailed {
                            component_id: Some(component_id.into()),
                            component_version,
                            reason,
                        },
                    ),
                ),
            },
            GolemError::ComponentParseFailed {
                component_id,
                component_version,
                reason,
            } => golem::worker::v1::WorkerExecutionError {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::ComponentParseFailed(
                        golem::worker::v1::ComponentParseFailed {
                            component_id: Some(component_id.into()),
                            component_version,
                            reason,
                        },
                    ),
                ),
            },
            GolemError::GetLatestVersionOfComponentFailed {
                component_id,
                reason,
            } => golem::worker::v1::WorkerExecutionError {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::GetLatestVersionOfComponentFailed(
                        golem::worker::v1::GetLatestVersionOfComponentFailed {
                            component_id: Some(component_id.into()),
                            reason,
                        },
                    ),
                ),
            },
            GolemError::InitialComponentFileDownloadFailed { path, reason } => {
                golem::worker::v1::WorkerExecutionError {
                    error: Some(
                        golem::worker::v1::worker_execution_error::Error::InitialComponentFileDownloadFailed(
                            golem::worker::v1::InitialComponentFileDownloadFailed { path, reason },
                        ),
                    ),
                }
            }
            GolemError::PromiseNotFound { promise_id } => golem::worker::v1::WorkerExecutionError {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::PromiseNotFound(
                        golem::worker::v1::PromiseNotFound {
                            promise_id: Some(promise_id.into()),
                        },
                    ),
                ),
            },
            GolemError::PromiseDropped { promise_id } => golem::worker::v1::WorkerExecutionError {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::PromiseDropped(
                        golem::worker::v1::PromiseDropped {
                            promise_id: Some(promise_id.into()),
                        },
                    ),
                ),
            },
            GolemError::PromiseAlreadyCompleted { promise_id } => {
                golem::worker::v1::WorkerExecutionError {
                    error: Some(
                        golem::worker::v1::worker_execution_error::Error::PromiseAlreadyCompleted(
                            golem::worker::v1::PromiseAlreadyCompleted {
                                promise_id: Some(promise_id.into()),
                            },
                        ),
                    ),
                }
            }
            GolemError::Interrupted { kind } => golem::worker::v1::WorkerExecutionError {
                error: Some(golem::worker::v1::worker_execution_error::Error::Interrupted(
                    golem::worker::v1::Interrupted {
                        recover_immediately: kind == InterruptKind::Restart,
                    },
                )),
            },
            GolemError::ParamTypeMismatch { details } => golem::worker::v1::WorkerExecutionError {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::ParamTypeMismatch(
                        golem::worker::v1::ParamTypeMismatch { details },
                    ),
                ),
            },
            GolemError::NoValueInMessage => golem::worker::v1::WorkerExecutionError {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::NoValueInMessage(
                        golem::worker::v1::NoValueInMessage {},
                    ),
                ),
            },
            GolemError::ValueMismatch { details } => golem::worker::v1::WorkerExecutionError {
                error: Some(golem::worker::v1::worker_execution_error::Error::ValueMismatch(
                    golem::worker::v1::ValueMismatch { details },
                )),
            },
            GolemError::UnexpectedOplogEntry { expected, got } => {
                golem::worker::v1::WorkerExecutionError {
                    error: Some(
                        golem::worker::v1::worker_execution_error::Error::UnexpectedOplogEntry(
                            golem::worker::v1::UnexpectedOplogEntry { expected, got },
                        ),
                    ),
                }
            }
            GolemError::Runtime { details } => golem::worker::v1::WorkerExecutionError {
                error: Some(golem::worker::v1::worker_execution_error::Error::RuntimeError(
                    golem::worker::v1::RuntimeError { details },
                )),
            },
            GolemError::InvalidShardId {
                shard_id,
                shard_ids,
            } => golem::worker::v1::WorkerExecutionError {
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
            GolemError::InvalidAccount => golem::worker::v1::WorkerExecutionError {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::InvalidAccount(
                        golem::worker::v1::InvalidAccount {},
                    ),
                ),
            },
            GolemError::PreviousInvocationFailed { details } => {
                golem::worker::v1::WorkerExecutionError {
                    error: Some(
                        golem::worker::v1::worker_execution_error::Error::PreviousInvocationFailed(
                            golem::worker::v1::PreviousInvocationFailed { details },
                        ),
                    ),
                }
            }
            GolemError::PreviousInvocationExited => golem::worker::v1::WorkerExecutionError {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::PreviousInvocationExited(
                        golem::worker::v1::PreviousInvocationExited {},
                    ),
                ),
            },
            GolemError::Unknown { details } => golem::worker::v1::WorkerExecutionError {
                error: Some(golem::worker::v1::worker_execution_error::Error::Unknown(
                    golem::worker::v1::UnknownError { details },
                )),
            },
            GolemError::ShardingNotReady => golem::worker::v1::WorkerExecutionError {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::ShardingNotReady(
                        golem::worker::v1::ShardingNotReady {},
                    ),
                ),
            },
            GolemError::FileSystemError { path, reason } => golem::worker::v1::WorkerExecutionError {
                error: Some(
                    golem::worker::v1::worker_execution_error::Error::FileSystemError(
                        golem::worker::v1::FileSystemError { path, reason },
                    ),
                ),
            },
        }
    }
}

impl TryFrom<golem::worker::v1::WorkerExecutionError> for GolemError {
    type Error = String;

    fn try_from(value: golem::worker::v1::WorkerExecutionError) -> Result<Self, Self::Error> {
        match value.error {
            None => Err("Unknown error".to_string()),
            Some(golem::worker::v1::worker_execution_error::Error::InvalidRequest(
                invalid_request,
            )) => Ok(GolemError::InvalidRequest {
                details: invalid_request.details,
            }),
            Some(golem::worker::v1::worker_execution_error::Error::WorkerAlreadyExists(
                worker_already_exists,
            )) => Ok(GolemError::WorkerAlreadyExists {
                worker_id: worker_already_exists
                    .worker_id
                    .ok_or("Missing worker_id")?
                    .try_into()?,
            }),
            Some(golem::worker::v1::worker_execution_error::Error::WorkerNotFound(
                worker_not_found,
            )) => Ok(GolemError::WorkerNotFound {
                worker_id: worker_not_found
                    .worker_id
                    .ok_or("Missing worker_id")?
                    .try_into()?,
            }),
            Some(golem::worker::v1::worker_execution_error::Error::WorkerCreationFailed(
                worker_creation_failed,
            )) => Ok(GolemError::WorkerCreationFailed {
                worker_id: worker_creation_failed
                    .worker_id
                    .ok_or("Missing worker_id")?
                    .try_into()?,
                details: worker_creation_failed.details,
            }),
            Some(golem::worker::v1::worker_execution_error::Error::FailedToResumeWorker(
                failed_to_resume_worker,
            )) => Ok(GolemError::FailedToResumeWorker {
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
            )) => Ok(GolemError::ComponentDownloadFailed {
                component_id: component_download_failed
                    .component_id
                    .ok_or("Missing component_id")?
                    .try_into()?,
                component_version: component_download_failed.component_version,
                reason: component_download_failed.reason,
            }),
            Some(golem::worker::v1::worker_execution_error::Error::ComponentParseFailed(
                component_parse_failed,
            )) => Ok(GolemError::ComponentParseFailed {
                component_id: component_parse_failed
                    .component_id
                    .ok_or("Missing component_id")?
                    .try_into()?,
                component_version: component_parse_failed.component_version,
                reason: component_parse_failed.reason,
            }),
            Some(
                golem::worker::v1::worker_execution_error::Error::GetLatestVersionOfComponentFailed(
                    get_latest_version_of_component_failed,
                ),
            ) => Ok(GolemError::GetLatestVersionOfComponentFailed {
                component_id: get_latest_version_of_component_failed
                    .component_id
                    .ok_or("Missing component_id")?
                    .try_into()?,
                reason: get_latest_version_of_component_failed.reason,
            }),
            Some(golem::worker::v1::worker_execution_error::Error::PromiseNotFound(
                promise_not_found,
            )) => Ok(GolemError::PromiseNotFound {
                promise_id: promise_not_found
                    .promise_id
                    .ok_or("Missing promise_id")?
                    .try_into()?,
            }),
            Some(golem::worker::v1::worker_execution_error::Error::PromiseDropped(
                promise_dropped,
            )) => Ok(GolemError::PromiseDropped {
                promise_id: promise_dropped
                    .promise_id
                    .ok_or("Missing promise_id")?
                    .try_into()?,
            }),
            Some(golem::worker::v1::worker_execution_error::Error::PromiseAlreadyCompleted(
                promise_already_completed,
            )) => Ok(GolemError::PromiseAlreadyCompleted {
                promise_id: promise_already_completed
                    .promise_id
                    .ok_or("Missing promise_id")?
                    .try_into()?,
            }),
            Some(golem::worker::v1::worker_execution_error::Error::Interrupted(interrupted)) => {
                Ok(GolemError::Interrupted {
                    kind: if interrupted.recover_immediately {
                        InterruptKind::Restart
                    } else {
                        InterruptKind::Interrupt
                    },
                })
            }
            Some(golem::worker::v1::worker_execution_error::Error::ParamTypeMismatch(_)) => {
                Ok(GolemError::ParamTypeMismatch {
                    details: "".to_string(),
                })
            }
            Some(golem::worker::v1::worker_execution_error::Error::NoValueInMessage(_)) => {
                Ok(GolemError::NoValueInMessage)
            }
            Some(golem::worker::v1::worker_execution_error::Error::ValueMismatch(
                value_mismatch,
            )) => Ok(GolemError::ValueMismatch {
                details: value_mismatch.details,
            }),
            Some(golem::worker::v1::worker_execution_error::Error::UnexpectedOplogEntry(
                unexpected_oplog_entry,
            )) => Ok(GolemError::UnexpectedOplogEntry {
                expected: unexpected_oplog_entry.expected,
                got: unexpected_oplog_entry.got,
            }),
            Some(golem::worker::v1::worker_execution_error::Error::InvalidShardId(
                invalid_shard_id,
            )) => Ok(GolemError::InvalidShardId {
                shard_id: invalid_shard_id.shard_id.ok_or("Missing shard_id")?.into(),
                shard_ids: invalid_shard_id
                    .shard_ids
                    .into_iter()
                    .map(|id| id.into())
                    .collect(),
            }),
            Some(golem::worker::v1::worker_execution_error::Error::InvalidAccount(_)) => {
                Ok(GolemError::InvalidAccount)
            }
            Some(golem::worker::v1::worker_execution_error::Error::RuntimeError(runtime_error)) => {
                Ok(GolemError::Runtime {
                    details: runtime_error.details,
                })
            }
            Some(golem::worker::v1::worker_execution_error::Error::PreviousInvocationFailed(
                previous_invocation_failed,
            )) => Ok(GolemError::PreviousInvocationFailed {
                details: previous_invocation_failed.details,
            }),
            Some(golem::worker::v1::worker_execution_error::Error::PreviousInvocationExited(_)) => {
                Ok(GolemError::PreviousInvocationExited)
            }
            Some(golem::worker::v1::worker_execution_error::Error::Unknown(unknown_error)) => {
                Ok(GolemError::Unknown {
                    details: unknown_error.details,
                })
            }
            Some(golem::worker::v1::worker_execution_error::Error::ShardingNotReady(_)) => {
                Ok(GolemError::ShardingNotReady)
            }
            Some(golem::worker::v1::worker_execution_error::Error::InitialComponentFileDownloadFailed(
                initial_file_download_failed,
            )) => Ok(GolemError::InitialComponentFileDownloadFailed {
                path: initial_file_download_failed.path,
                reason: initial_file_download_failed.reason,
            }),
            Some(golem::worker::v1::worker_execution_error::Error::FileSystemError(
                file_system_error,
            )) => Ok(GolemError::FileSystemError {
                path: file_system_error.path,
                reason: file_system_error.reason,
            }),
        }
    }
}

impl From<EncodingError> for GolemError {
    fn from(value: EncodingError) -> Self {
        match value {
            EncodingError::ParamTypeMismatch { details } => {
                GolemError::ParamTypeMismatch { details }
            }
            EncodingError::ValueMismatch { details } => GolemError::ValueMismatch { details },
            EncodingError::Unknown { details } => GolemError::Unknown { details },
        }
    }
}

#[derive(Debug, Clone, PartialOrd, PartialEq, Eq, Hash)]
pub struct WorkerOutOfMemory;

impl Display for WorkerOutOfMemory {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "Worker cannot acquire more memory")
    }
}

impl Error for WorkerOutOfMemory {}
