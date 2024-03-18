// Copyright 2024 Golem Cloud
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
use golem_wasm_rpc::wasmtime::EncodingError;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::error::Error;
use std::fmt::{Display, Formatter};

use golem_api_grpc::proto::golem;
use golem_common::model::{PromiseId, ShardId, TemplateId, WorkerId};
use tonic::Status;

use crate::model::InterruptKind;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Encode, Decode)]
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
    },
    TemplateDownloadFailed {
        template_id: TemplateId,
        template_version: i32,
        reason: String,
    },
    TemplateParseFailed {
        template_id: TemplateId,
        template_version: i32,
        reason: String,
    },
    GetLatestVersionOfTemplateFailed {
        template_id: TemplateId,
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
    ParamTypeMismatch,
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
    PreviousInvocationFailed,
    PreviousInvocationExited,
    Unknown {
        details: String,
    },
}

impl GolemError {
    pub fn failed_to_resume_instance(worker_id: WorkerId) -> Self {
        GolemError::FailedToResumeWorker { worker_id }
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

    pub fn template_download_failed(
        template_id: TemplateId,
        template_version: i32,
        reason: impl Into<String>,
    ) -> Self {
        GolemError::TemplateDownloadFailed {
            template_id,
            template_version,
            reason: reason.into(),
        }
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

    pub fn kind(&self) -> &'static str {
        match self {
            GolemError::InvalidRequest { .. } => "InvalidRequest",
            GolemError::WorkerAlreadyExists { .. } => "WorkerAlreadyExists",
            GolemError::WorkerNotFound { .. } => "WorkerNotFound",
            GolemError::WorkerCreationFailed { .. } => "WorkerCreationFailed",
            GolemError::FailedToResumeWorker { .. } => "FailedToResumeWorker",
            GolemError::TemplateDownloadFailed { .. } => "TemplateDownloadFailed",
            GolemError::TemplateParseFailed { .. } => "TemplateParseFailed",
            GolemError::GetLatestVersionOfTemplateFailed { .. } => {
                "GetLatestVersionOfTemplateFailed"
            }
            GolemError::PromiseNotFound { .. } => "PromiseNotFound",
            GolemError::PromiseDropped { .. } => "PromiseDropped",
            GolemError::PromiseAlreadyCompleted { .. } => "PromiseAlreadyCompleted",
            GolemError::Interrupted { .. } => "Interrupted",
            GolemError::ParamTypeMismatch => "ParamTypeMismatch",
            GolemError::NoValueInMessage => "NoValueInMessage",
            GolemError::ValueMismatch { .. } => "ValueMismatch",
            GolemError::UnexpectedOplogEntry { .. } => "UnexpectedOplogEntry",
            GolemError::InvalidShardId { .. } => "InvalidShardId",
            GolemError::InvalidAccount => "InvalidAccount",
            GolemError::Runtime { .. } => "Runtime",
            GolemError::PreviousInvocationFailed => "PreviousInvocationFailed",
            GolemError::PreviousInvocationExited => "PreviousInvocationExited",
            GolemError::Unknown { .. } => "Unknown",
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
            GolemError::FailedToResumeWorker { worker_id } => {
                write!(f, "Failed to resume worker: {worker_id}")
            }
            GolemError::TemplateDownloadFailed {
                template_id,
                template_version,
                reason,
            } => {
                write!(
                    f,
                    "Failed to download template: {template_id}#{template_version}: {reason}"
                )
            }
            GolemError::TemplateParseFailed {
                template_id,
                template_version,
                reason,
            } => {
                write!(
                    f,
                    "Failed to parse downloaded template: {template_id}#{template_version}: {reason}"
                )
            }
            GolemError::GetLatestVersionOfTemplateFailed {
                template_id,
                reason,
            } => {
                write!(
                    f,
                    "Failed to get latest version of template {template_id}: {reason}"
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
            GolemError::ParamTypeMismatch => {
                write!(f, "Parameter type mismatch")
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
            GolemError::PreviousInvocationFailed => {
                write!(f, "The previously invoked function failed")
            }
            GolemError::PreviousInvocationExited => {
                write!(f, "The previously invoked function exited")
            }
            GolemError::Unknown { details } => {
                write!(f, "Unknown error: {details}")
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
            GolemError::TemplateDownloadFailed { .. } => "Failed to download template",
            GolemError::TemplateParseFailed { .. } => "Failed to parse downloaded template",
            GolemError::GetLatestVersionOfTemplateFailed { .. } => {
                "Failed to get latest version of template"
            }
            GolemError::PromiseNotFound { .. } => "Promise not found",
            GolemError::PromiseDropped { .. } => "Promise dropped",
            GolemError::PromiseAlreadyCompleted { .. } => "Promise already completed",
            GolemError::Interrupted { .. } => "Interrupted",
            GolemError::ParamTypeMismatch => "Parameter type mismatch",
            GolemError::NoValueInMessage => "No value in message",
            GolemError::ValueMismatch { .. } => "Value mismatch",
            GolemError::UnexpectedOplogEntry { .. } => "Unexpected oplog entry",
            GolemError::InvalidShardId { .. } => "Invalid shard",
            GolemError::InvalidAccount => "Invalid account",
            GolemError::Runtime { .. } => "Runtime error",
            GolemError::PreviousInvocationFailed => "The previously invoked function failed",
            GolemError::PreviousInvocationExited => "The previously invoked function exited",
            GolemError::Unknown { .. } => "Unknown error",
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
            None => GolemError::runtime(format!("{error}")),
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
            GolemError::ParamTypeMismatch => {
                Status::invalid_argument("Parameter type mismatch".to_string())
            }
            GolemError::NoValueInMessage => {
                Status::invalid_argument("No value in message".to_string())
            }
            GolemError::ValueMismatch { details } => {
                Status::invalid_argument(format!("Value mismatch: {details}", details = details))
            }
            GolemError::Unknown { details } => Status::unknown(details),
            _ => Status::internal(format!("{value}")),
        }
    }
}

impl From<GolemError> for golem::worker::WorkerExecutionError {
    fn from(value: GolemError) -> Self {
        match value {
            GolemError::InvalidRequest { details } => golem::worker::WorkerExecutionError {
                error: Some(
                    golem::worker::worker_execution_error::Error::InvalidRequest(
                        golem::worker::InvalidRequest { details },
                    ),
                ),
            },
            GolemError::WorkerAlreadyExists { worker_id } => golem::worker::WorkerExecutionError {
                error: Some(
                    golem::worker::worker_execution_error::Error::WorkerAlreadyExists(
                        golem::worker::WorkerAlreadyExists {
                            worker_id: Some(worker_id.into_proto()),
                        },
                    ),
                ),
            },
            GolemError::WorkerNotFound { worker_id } => golem::worker::WorkerExecutionError {
                error: Some(
                    golem::worker::worker_execution_error::Error::WorkerNotFound(
                        golem::worker::WorkerNotFound {
                            worker_id: Some(worker_id.into_proto()),
                        },
                    ),
                ),
            },
            GolemError::WorkerCreationFailed { worker_id, details } => {
                golem::worker::WorkerExecutionError {
                    error: Some(
                        golem::worker::worker_execution_error::Error::WorkerCreationFailed(
                            golem::worker::WorkerCreationFailed {
                                worker_id: Some(worker_id.into_proto()),
                                details,
                            },
                        ),
                    ),
                }
            }
            GolemError::FailedToResumeWorker { worker_id } => golem::worker::WorkerExecutionError {
                error: Some(
                    golem::worker::worker_execution_error::Error::FailedToResumeWorker(
                        golem::worker::FailedToResumeWorker {
                            worker_id: Some(worker_id.into_proto()),
                        },
                    ),
                ),
            },
            GolemError::TemplateDownloadFailed {
                template_id,
                template_version,
                reason,
            } => golem::worker::WorkerExecutionError {
                error: Some(
                    golem::worker::worker_execution_error::Error::TemplateDownloadFailed(
                        golem::worker::TemplateDownloadFailed {
                            template_id: Some(template_id.into()),
                            template_version,
                            reason,
                        },
                    ),
                ),
            },
            GolemError::TemplateParseFailed {
                template_id,
                template_version,
                reason,
            } => golem::worker::WorkerExecutionError {
                error: Some(
                    golem::worker::worker_execution_error::Error::TemplateParseFailed(
                        golem::worker::TemplateParseFailed {
                            template_id: Some(template_id.into()),
                            template_version,
                            reason,
                        },
                    ),
                ),
            },
            GolemError::GetLatestVersionOfTemplateFailed {
                template_id,
                reason,
            } => golem::worker::WorkerExecutionError {
                error: Some(
                    golem::worker::worker_execution_error::Error::GetLatestVersionOfTemplateFailed(
                        golem::worker::GetLatestVersionOfTemplateFailed {
                            template_id: Some(template_id.into()),
                            reason,
                        },
                    ),
                ),
            },
            GolemError::PromiseNotFound { promise_id } => golem::worker::WorkerExecutionError {
                error: Some(
                    golem::worker::worker_execution_error::Error::PromiseNotFound(
                        golem::worker::PromiseNotFound {
                            promise_id: Some(promise_id.into()),
                        },
                    ),
                ),
            },
            GolemError::PromiseDropped { promise_id } => golem::worker::WorkerExecutionError {
                error: Some(
                    golem::worker::worker_execution_error::Error::PromiseDropped(
                        golem::worker::PromiseDropped {
                            promise_id: Some(promise_id.into()),
                        },
                    ),
                ),
            },
            GolemError::PromiseAlreadyCompleted { promise_id } => {
                golem::worker::WorkerExecutionError {
                    error: Some(
                        golem::worker::worker_execution_error::Error::PromiseAlreadyCompleted(
                            golem::worker::PromiseAlreadyCompleted {
                                promise_id: Some(promise_id.into()),
                            },
                        ),
                    ),
                }
            }
            GolemError::Interrupted { kind } => golem::worker::WorkerExecutionError {
                error: Some(golem::worker::worker_execution_error::Error::Interrupted(
                    golem::worker::Interrupted {
                        recover_immediately: kind == InterruptKind::Restart,
                    },
                )),
            },
            GolemError::ParamTypeMismatch => golem::worker::WorkerExecutionError {
                error: Some(
                    golem::worker::worker_execution_error::Error::ParamTypeMismatch(
                        golem::worker::ParamTypeMismatch {},
                    ),
                ),
            },
            GolemError::NoValueInMessage => golem::worker::WorkerExecutionError {
                error: Some(
                    golem::worker::worker_execution_error::Error::NoValueInMessage(
                        golem::worker::NoValueInMessage {},
                    ),
                ),
            },
            GolemError::ValueMismatch { details } => golem::worker::WorkerExecutionError {
                error: Some(golem::worker::worker_execution_error::Error::ValueMismatch(
                    golem::worker::ValueMismatch { details },
                )),
            },
            GolemError::UnexpectedOplogEntry { expected, got } => {
                golem::worker::WorkerExecutionError {
                    error: Some(
                        golem::worker::worker_execution_error::Error::UnexpectedOplogEntry(
                            golem::worker::UnexpectedOplogEntry { expected, got },
                        ),
                    ),
                }
            }
            GolemError::Runtime { details } => golem::worker::WorkerExecutionError {
                error: Some(golem::worker::worker_execution_error::Error::RuntimeError(
                    golem::worker::RuntimeError { details },
                )),
            },
            GolemError::InvalidShardId {
                shard_id,
                shard_ids,
            } => golem::worker::WorkerExecutionError {
                error: Some(
                    golem::worker::worker_execution_error::Error::InvalidShardId(
                        golem::worker::InvalidShardId {
                            shard_id: Some(shard_id.into()),
                            shard_ids: shard_ids
                                .into_iter()
                                .map(|shard_id| shard_id.into())
                                .collect(),
                        },
                    ),
                ),
            },
            GolemError::InvalidAccount => golem::worker::WorkerExecutionError {
                error: Some(
                    golem::worker::worker_execution_error::Error::InvalidAccount(
                        golem::worker::InvalidAccount {},
                    ),
                ),
            },
            GolemError::PreviousInvocationFailed => golem::worker::WorkerExecutionError {
                error: Some(
                    golem::worker::worker_execution_error::Error::PreviousInvocationFailed(
                        golem::worker::PreviousInvocationFailed {},
                    ),
                ),
            },
            GolemError::PreviousInvocationExited => golem::worker::WorkerExecutionError {
                error: Some(
                    golem::worker::worker_execution_error::Error::PreviousInvocationExited(
                        golem::worker::PreviousInvocationExited {},
                    ),
                ),
            },
            GolemError::Unknown { details } => golem::worker::WorkerExecutionError {
                error: Some(golem::worker::worker_execution_error::Error::Unknown(
                    golem::worker::UnknownError { details },
                )),
            },
        }
    }
}

impl TryFrom<golem::worker::WorkerExecutionError> for GolemError {
    type Error = String;

    fn try_from(value: golem::worker::WorkerExecutionError) -> Result<Self, Self::Error> {
        match value.error {
            None => Err("Unknown error".to_string()),
            Some(golem::worker::worker_execution_error::Error::InvalidRequest(invalid_request)) => {
                Ok(GolemError::InvalidRequest {
                    details: invalid_request.details,
                })
            }
            Some(golem::worker::worker_execution_error::Error::WorkerAlreadyExists(
                worker_already_exists,
            )) => Ok(GolemError::WorkerAlreadyExists {
                worker_id: worker_already_exists
                    .worker_id
                    .ok_or("Missing worker_id")?
                    .try_into()?,
            }),
            Some(golem::worker::worker_execution_error::Error::WorkerNotFound(
                worker_not_found,
            )) => Ok(GolemError::WorkerNotFound {
                worker_id: worker_not_found
                    .worker_id
                    .ok_or("Missing worker_id")?
                    .try_into()?,
            }),
            Some(golem::worker::worker_execution_error::Error::WorkerCreationFailed(
                worker_creation_failed,
            )) => Ok(GolemError::WorkerCreationFailed {
                worker_id: worker_creation_failed
                    .worker_id
                    .ok_or("Missing worker_id")?
                    .try_into()?,
                details: worker_creation_failed.details,
            }),
            Some(golem::worker::worker_execution_error::Error::FailedToResumeWorker(
                failed_to_resume_worker,
            )) => Ok(GolemError::FailedToResumeWorker {
                worker_id: failed_to_resume_worker
                    .worker_id
                    .ok_or("Missing worker_id")?
                    .try_into()?,
            }),
            Some(golem::worker::worker_execution_error::Error::TemplateDownloadFailed(
                template_download_failed,
            )) => Ok(GolemError::TemplateDownloadFailed {
                template_id: template_download_failed
                    .template_id
                    .ok_or("Missing template_id")?
                    .try_into()?,
                template_version: template_download_failed.template_version,
                reason: template_download_failed.reason,
            }),
            Some(golem::worker::worker_execution_error::Error::TemplateParseFailed(
                template_parse_failed,
            )) => Ok(GolemError::TemplateParseFailed {
                template_id: template_parse_failed
                    .template_id
                    .ok_or("Missing template_id")?
                    .try_into()?,
                template_version: template_parse_failed.template_version,
                reason: template_parse_failed.reason,
            }),
            Some(
                golem::worker::worker_execution_error::Error::GetLatestVersionOfTemplateFailed(
                    get_latest_version_of_template_failed,
                ),
            ) => Ok(GolemError::GetLatestVersionOfTemplateFailed {
                template_id: get_latest_version_of_template_failed
                    .template_id
                    .ok_or("Missing template_id")?
                    .try_into()?,
                reason: get_latest_version_of_template_failed.reason,
            }),
            Some(golem::worker::worker_execution_error::Error::PromiseNotFound(
                promise_not_found,
            )) => Ok(GolemError::PromiseNotFound {
                promise_id: promise_not_found
                    .promise_id
                    .ok_or("Missing promise_id")?
                    .try_into()?,
            }),
            Some(golem::worker::worker_execution_error::Error::PromiseDropped(promise_dropped)) => {
                Ok(GolemError::PromiseDropped {
                    promise_id: promise_dropped
                        .promise_id
                        .ok_or("Missing promise_id")?
                        .try_into()?,
                })
            }
            Some(golem::worker::worker_execution_error::Error::PromiseAlreadyCompleted(
                promise_already_completed,
            )) => Ok(GolemError::PromiseAlreadyCompleted {
                promise_id: promise_already_completed
                    .promise_id
                    .ok_or("Missing promise_id")?
                    .try_into()?,
            }),
            Some(golem::worker::worker_execution_error::Error::Interrupted(interrupted)) => {
                Ok(GolemError::Interrupted {
                    kind: if interrupted.recover_immediately {
                        InterruptKind::Restart
                    } else {
                        InterruptKind::Interrupt
                    },
                })
            }
            Some(golem::worker::worker_execution_error::Error::ParamTypeMismatch(_)) => {
                Ok(GolemError::ParamTypeMismatch)
            }
            Some(golem::worker::worker_execution_error::Error::NoValueInMessage(_)) => {
                Ok(GolemError::NoValueInMessage)
            }
            Some(golem::worker::worker_execution_error::Error::ValueMismatch(value_mismatch)) => {
                Ok(GolemError::ValueMismatch {
                    details: value_mismatch.details,
                })
            }
            Some(golem::worker::worker_execution_error::Error::UnexpectedOplogEntry(
                unexpected_oplog_entry,
            )) => Ok(GolemError::UnexpectedOplogEntry {
                expected: unexpected_oplog_entry.expected,
                got: unexpected_oplog_entry.got,
            }),
            Some(golem::worker::worker_execution_error::Error::InvalidShardId(
                invalid_shard_id,
            )) => Ok(GolemError::InvalidShardId {
                shard_id: invalid_shard_id.shard_id.ok_or("Missing shard_id")?.into(),
                shard_ids: invalid_shard_id
                    .shard_ids
                    .into_iter()
                    .map(|id| id.into())
                    .collect(),
            }),
            Some(golem::worker::worker_execution_error::Error::InvalidAccount(_)) => {
                Ok(GolemError::InvalidAccount)
            }
            Some(golem::worker::worker_execution_error::Error::RuntimeError(runtime_error)) => {
                Ok(GolemError::Runtime {
                    details: runtime_error.details,
                })
            }
            Some(golem::worker::worker_execution_error::Error::PreviousInvocationFailed(_)) => {
                Ok(GolemError::PreviousInvocationFailed)
            }
            Some(golem::worker::worker_execution_error::Error::PreviousInvocationExited(_)) => {
                Ok(GolemError::PreviousInvocationExited)
            }
            Some(golem::worker::worker_execution_error::Error::Unknown(unknown_error)) => {
                Ok(GolemError::Unknown {
                    details: unknown_error.details,
                })
            }
        }
    }
}

impl From<EncodingError> for GolemError {
    fn from(value: EncodingError) -> Self {
        match value {
            EncodingError::ParamTypeMismatch => GolemError::ParamTypeMismatch,
            EncodingError::ValueMismatch { details } => GolemError::ValueMismatch { details },
            EncodingError::Unknown { details } => GolemError::Unknown { details },
        }
    }
}

pub fn is_interrupt(error: &anyhow::Error) -> bool {
    error
        .root_cause()
        .downcast_ref::<InterruptKind>()
        .map_or(false, |kind| *kind == InterruptKind::Interrupt)
}

pub fn is_suspend(error: &anyhow::Error) -> bool {
    error
        .root_cause()
        .downcast_ref::<InterruptKind>()
        .map_or(false, |kind| *kind == InterruptKind::Suspend)
}

pub fn is_jump(error: &anyhow::Error) -> bool {
    error
        .root_cause()
        .downcast_ref::<InterruptKind>()
        .map_or(false, |kind| *kind == InterruptKind::Jump)
}
