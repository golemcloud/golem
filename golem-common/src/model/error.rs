use crate::base_model::{ComponentId, PromiseId, ShardId, WorkerId};
use crate::model::component::VersionedComponentId;
use crate::SafeDisplay;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[error("Invalid request: {details}")]
pub struct GolemErrorInvalidRequest {
    pub details: String,
}

impl SafeDisplay for GolemErrorInvalidRequest {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[error("Worker already exists: {worker_id}")]
pub struct GolemErrorWorkerAlreadyExists {
    pub worker_id: WorkerId,
}

impl SafeDisplay for GolemErrorWorkerAlreadyExists {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[error("Worker not found: {worker_id}")]
pub struct GolemErrorWorkerNotFound {
    pub worker_id: WorkerId,
}

impl SafeDisplay for GolemErrorWorkerNotFound {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[error("Worker creation failed {worker_id}: {details}")]
pub struct GolemErrorWorkerCreationFailed {
    pub worker_id: WorkerId,
    pub details: String,
}

impl SafeDisplay for GolemErrorWorkerCreationFailed {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[error("Failed to resume worker: {worker_id}")]
pub struct GolemErrorFailedToResumeWorker {
    pub worker_id: WorkerId,
    pub reason: Box<GolemError>,
}

impl SafeDisplay for GolemErrorFailedToResumeWorker {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[error("Failed to download component {component_id}: {reason}")]
pub struct GolemErrorComponentDownloadFailed {
    pub component_id: VersionedComponentId,
    pub reason: String,
}

impl SafeDisplay for GolemErrorComponentDownloadFailed {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[error("Failed to parse component {component_id}: {reason}")]
pub struct GolemErrorComponentParseFailed {
    pub component_id: VersionedComponentId,
    pub reason: String,
}

impl SafeDisplay for GolemErrorComponentParseFailed {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[error("Failed to get latest version of component {component_id}: {reason}")]
pub struct GolemErrorGetLatestVersionOfComponentFailed {
    pub component_id: ComponentId,
    pub reason: String,
}

impl SafeDisplay for GolemErrorGetLatestVersionOfComponentFailed {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[error("Failed to find promise: {promise_id}")]
pub struct GolemErrorPromiseNotFound {
    pub promise_id: PromiseId,
}

impl SafeDisplay for GolemErrorPromiseNotFound {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[error("Promise dropped: {promise_id}")]
pub struct GolemErrorPromiseDropped {
    pub promise_id: PromiseId,
}

impl SafeDisplay for GolemErrorPromiseDropped {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[error("Promise already completed: {promise_id}")]
pub struct GolemErrorPromiseAlreadyCompleted {
    pub promise_id: PromiseId,
}

impl SafeDisplay for GolemErrorPromiseAlreadyCompleted {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[error(
    "Worker Interrupted: {}", if *.recover_immediately { "recovering immediately" } else { "not recovering immediately" }
)]
pub struct GolemErrorInterrupted {
    pub recover_immediately: bool,
}

impl SafeDisplay for GolemErrorInterrupted {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[error("Parameter type mismatch")]
pub struct GolemErrorParamTypeMismatch {
    pub details: String,
}

impl SafeDisplay for GolemErrorParamTypeMismatch {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[error("No value in message")]
pub struct GolemErrorNoValueInMessage {}

impl SafeDisplay for GolemErrorNoValueInMessage {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[error("Value mismatch: {details}")]
pub struct GolemErrorValueMismatch {
    pub details: String,
}

impl SafeDisplay for GolemErrorValueMismatch {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[error("Unexpected oplog entry: expected {expected}, got {got}")]
pub struct GolemErrorUnexpectedOplogEntry {
    pub expected: String,
    pub got: String,
}

impl SafeDisplay for GolemErrorUnexpectedOplogEntry {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[error("Runtime error: {details}")]
pub struct GolemErrorRuntimeError {
    pub details: String,
}

impl SafeDisplay for GolemErrorRuntimeError {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[error("Invalid shard id: {shard_id}, valid shard ids: {shard_ids:?}")]
pub struct GolemErrorInvalidShardId {
    pub shard_id: ShardId,
    pub shard_ids: std::collections::HashSet<ShardId>,
}

impl SafeDisplay for GolemErrorInvalidShardId {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[error("Previous invocation failed: {details}")]
pub struct GolemErrorPreviousInvocationFailed {
    pub details: String,
}

impl SafeDisplay for GolemErrorPreviousInvocationFailed {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[error("Previous invocation exited")]
pub struct GolemErrorPreviousInvocationExited {}

impl SafeDisplay for GolemErrorPreviousInvocationExited {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[error("Unknown error: {details}")]
pub struct GolemErrorUnknown {
    pub details: String,
}

impl SafeDisplay for GolemErrorUnknown {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[error("Invalid account")]
pub struct GolemErrorInvalidAccount {}

impl SafeDisplay for GolemErrorInvalidAccount {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[error("Initial file upload failed")]
pub struct GolemErrorInitialComponentFileUploadFailed {
    pub component_id: VersionedComponentId,
    pub path: String,
    pub reason: String,
}

impl SafeDisplay for GolemErrorInitialComponentFileUploadFailed {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[error("Initial file download failed")]
pub struct GolemErrorInitialComponentFileDownloadFailed {
    pub path: String,
    pub reason: String,
}

impl SafeDisplay for GolemErrorInitialComponentFileDownloadFailed {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[error("Error with accessing worker files")]
pub struct GolemErrorFileSystemError {
    pub path: String,
    pub reason: String,
}

impl SafeDisplay for GolemErrorFileSystemError {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
#[error("Invalid account")]
pub struct GolemErrorShardingNotReady {}

impl SafeDisplay for GolemErrorShardingNotReady {
    fn to_safe_string(&self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Union))]
#[cfg_attr(feature = "poem", oai(discriminator_name = "type", one_of = true))]
#[serde(tag = "type")]
pub enum GolemError {
    #[error(transparent)]
    InvalidRequest(GolemErrorInvalidRequest),
    #[error(transparent)]
    WorkerAlreadyExists(GolemErrorWorkerAlreadyExists),
    #[error(transparent)]
    WorkerNotFound(GolemErrorWorkerNotFound),
    #[error(transparent)]
    WorkerCreationFailed(GolemErrorWorkerCreationFailed),
    #[error(transparent)]
    FailedToResumeWorker(GolemErrorFailedToResumeWorker),
    #[error(transparent)]
    ComponentDownloadFailed(GolemErrorComponentDownloadFailed),
    #[error(transparent)]
    ComponentParseFailed(GolemErrorComponentParseFailed),
    #[error(transparent)]
    GetLatestVersionOfComponentFailed(GolemErrorGetLatestVersionOfComponentFailed),
    #[error(transparent)]
    PromiseNotFound(GolemErrorPromiseNotFound),
    #[error(transparent)]
    PromiseDropped(GolemErrorPromiseDropped),
    #[error(transparent)]
    PromiseAlreadyCompleted(GolemErrorPromiseAlreadyCompleted),
    #[error(transparent)]
    Interrupted(GolemErrorInterrupted),
    #[error(transparent)]
    ParamTypeMismatch(GolemErrorParamTypeMismatch),
    #[error(transparent)]
    NoValueInMessage(GolemErrorNoValueInMessage),
    #[error(transparent)]
    ValueMismatch(GolemErrorValueMismatch),
    #[error(transparent)]
    UnexpectedOplogEntry(GolemErrorUnexpectedOplogEntry),
    #[error(transparent)]
    RuntimeError(GolemErrorRuntimeError),
    #[error(transparent)]
    InvalidShardId(GolemErrorInvalidShardId),
    #[error(transparent)]
    PreviousInvocationFailed(GolemErrorPreviousInvocationFailed),
    #[error(transparent)]
    PreviousInvocationExited(GolemErrorPreviousInvocationExited),
    #[error(transparent)]
    Unknown(GolemErrorUnknown),
    #[error(transparent)]
    InvalidAccount(GolemErrorInvalidAccount),
    #[error(transparent)]
    ShardingNotReady(GolemErrorShardingNotReady),
    #[error(transparent)]
    InitialComponentFileDownloadFailed(GolemErrorInitialComponentFileDownloadFailed),
    #[error(transparent)]
    FileSystemError(GolemErrorFileSystemError),
}

impl SafeDisplay for GolemError {
    fn to_safe_string(&self) -> String {
        match self {
            GolemError::InvalidRequest(inner) => inner.to_safe_string(),
            GolemError::WorkerAlreadyExists(inner) => inner.to_safe_string(),
            GolemError::WorkerNotFound(inner) => inner.to_safe_string(),
            GolemError::WorkerCreationFailed(inner) => inner.to_safe_string(),
            GolemError::FailedToResumeWorker(inner) => inner.to_safe_string(),
            GolemError::ComponentDownloadFailed(inner) => inner.to_safe_string(),
            GolemError::ComponentParseFailed(inner) => inner.to_safe_string(),
            GolemError::GetLatestVersionOfComponentFailed(inner) => inner.to_safe_string(),
            GolemError::PromiseNotFound(inner) => inner.to_safe_string(),
            GolemError::PromiseDropped(inner) => inner.to_safe_string(),
            GolemError::PromiseAlreadyCompleted(inner) => inner.to_safe_string(),
            GolemError::Interrupted(inner) => inner.to_safe_string(),
            GolemError::ParamTypeMismatch(inner) => inner.to_safe_string(),
            GolemError::NoValueInMessage(inner) => inner.to_safe_string(),
            GolemError::ValueMismatch(inner) => inner.to_safe_string(),
            GolemError::UnexpectedOplogEntry(inner) => inner.to_safe_string(),
            GolemError::RuntimeError(inner) => inner.to_safe_string(),
            GolemError::InvalidShardId(inner) => inner.to_safe_string(),
            GolemError::PreviousInvocationFailed(inner) => inner.to_safe_string(),
            GolemError::PreviousInvocationExited(inner) => inner.to_safe_string(),
            GolemError::Unknown(inner) => inner.to_safe_string(),
            GolemError::InvalidAccount(inner) => inner.to_safe_string(),
            GolemError::ShardingNotReady(inner) => inner.to_safe_string(),
            GolemError::InitialComponentFileDownloadFailed(inner) => inner.to_safe_string(),
            GolemError::FileSystemError(inner) => inner.to_safe_string(),
        }
    }
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
pub struct GolemErrorBody {
    pub golem_error: GolemError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
pub struct ErrorsBody {
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
pub struct ErrorBody {
    pub error: String,
}

#[cfg(feature = "protobuf")]
mod protobuf {
    use crate::model::component::VersionedComponentId;
    use crate::model::error::{
        ErrorBody, ErrorsBody, GolemError, GolemErrorBody, GolemErrorComponentDownloadFailed,
        GolemErrorComponentParseFailed, GolemErrorFailedToResumeWorker, GolemErrorFileSystemError,
        GolemErrorGetLatestVersionOfComponentFailed, GolemErrorInitialComponentFileDownloadFailed,
        GolemErrorInterrupted, GolemErrorInvalidAccount, GolemErrorInvalidRequest,
        GolemErrorInvalidShardId, GolemErrorNoValueInMessage, GolemErrorParamTypeMismatch,
        GolemErrorPreviousInvocationExited, GolemErrorPreviousInvocationFailed,
        GolemErrorPromiseAlreadyCompleted, GolemErrorPromiseDropped, GolemErrorPromiseNotFound,
        GolemErrorRuntimeError, GolemErrorShardingNotReady, GolemErrorUnexpectedOplogEntry,
        GolemErrorUnknown, GolemErrorValueMismatch, GolemErrorWorkerAlreadyExists,
        GolemErrorWorkerCreationFailed, GolemErrorWorkerNotFound,
    };

    impl From<golem_api_grpc::proto::golem::worker::v1::InvalidRequest> for GolemErrorInvalidRequest {
        fn from(value: golem_api_grpc::proto::golem::worker::v1::InvalidRequest) -> Self {
            Self {
                details: value.details,
            }
        }
    }

    impl From<GolemErrorInvalidRequest> for golem_api_grpc::proto::golem::worker::v1::InvalidRequest {
        fn from(value: GolemErrorInvalidRequest) -> Self {
            Self {
                details: value.details,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::worker::v1::WorkerAlreadyExists>
        for GolemErrorWorkerAlreadyExists
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::worker::v1::WorkerAlreadyExists,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                worker_id: value
                    .worker_id
                    .ok_or("Missing field: worker_id")?
                    .try_into()?,
            })
        }
    }

    impl From<GolemErrorWorkerAlreadyExists>
        for golem_api_grpc::proto::golem::worker::v1::WorkerAlreadyExists
    {
        fn from(value: GolemErrorWorkerAlreadyExists) -> Self {
            Self {
                worker_id: Some(value.worker_id.into()),
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::worker::v1::WorkerNotFound>
        for GolemErrorWorkerNotFound
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::worker::v1::WorkerNotFound,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                worker_id: value
                    .worker_id
                    .ok_or("Missing field: worker_id")?
                    .try_into()?,
            })
        }
    }

    impl From<GolemErrorWorkerNotFound> for golem_api_grpc::proto::golem::worker::v1::WorkerNotFound {
        fn from(value: GolemErrorWorkerNotFound) -> Self {
            Self {
                worker_id: Some(value.worker_id.into()),
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::worker::v1::WorkerCreationFailed>
        for GolemErrorWorkerCreationFailed
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::worker::v1::WorkerCreationFailed,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                worker_id: value
                    .worker_id
                    .ok_or("Missing field: worker_id")?
                    .try_into()?,
                details: value.details,
            })
        }
    }

    impl From<GolemErrorWorkerCreationFailed>
        for golem_api_grpc::proto::golem::worker::v1::WorkerCreationFailed
    {
        fn from(value: GolemErrorWorkerCreationFailed) -> Self {
            Self {
                worker_id: Some(value.worker_id.into()),
                details: value.details,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::worker::v1::FailedToResumeWorker>
        for GolemErrorFailedToResumeWorker
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::worker::v1::FailedToResumeWorker,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                worker_id: value
                    .worker_id
                    .ok_or("Missing field: worker_id")?
                    .try_into()?,
                reason: Box::new((*value.reason.ok_or("Missing field: reason")?).try_into()?),
            })
        }
    }

    impl From<GolemErrorFailedToResumeWorker>
        for golem_api_grpc::proto::golem::worker::v1::FailedToResumeWorker
    {
        fn from(value: GolemErrorFailedToResumeWorker) -> Self {
            Self {
                worker_id: Some(value.worker_id.into()),
                reason: Some(Box::new((*value.reason).into())),
            }
        }
    }

    impl From<golem_api_grpc::proto::golem::worker::v1::InvalidAccount> for GolemErrorInvalidAccount {
        fn from(_value: golem_api_grpc::proto::golem::worker::v1::InvalidAccount) -> Self {
            Self {}
        }
    }

    impl From<GolemErrorInvalidAccount> for golem_api_grpc::proto::golem::worker::v1::InvalidAccount {
        fn from(_value: GolemErrorInvalidAccount) -> Self {
            Self {}
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::worker::v1::ComponentDownloadFailed>
        for GolemErrorComponentDownloadFailed
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::worker::v1::ComponentDownloadFailed,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                component_id: VersionedComponentId {
                    component_id: value
                        .component_id
                        .ok_or("Missing field: component_id")?
                        .try_into()?,
                    version: value.component_version,
                },
                reason: value.reason,
            })
        }
    }

    impl From<GolemErrorComponentDownloadFailed>
        for golem_api_grpc::proto::golem::worker::v1::ComponentDownloadFailed
    {
        fn from(value: GolemErrorComponentDownloadFailed) -> Self {
            let component_version = value.component_id.version;
            let component_id = golem_api_grpc::proto::golem::component::ComponentId {
                value: Some(value.component_id.component_id.0.into()),
            };
            Self {
                component_id: Some(component_id),
                component_version,
                reason: value.reason,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::worker::v1::ComponentParseFailed>
        for GolemErrorComponentParseFailed
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::worker::v1::ComponentParseFailed,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                component_id: VersionedComponentId {
                    component_id: value
                        .component_id
                        .ok_or("Missing field: component_id")?
                        .try_into()?,
                    version: value.component_version,
                },
                reason: value.reason,
            })
        }
    }

    impl From<GolemErrorComponentParseFailed>
        for golem_api_grpc::proto::golem::worker::v1::ComponentParseFailed
    {
        fn from(value: GolemErrorComponentParseFailed) -> Self {
            let component_version = value.component_id.version;
            let component_id = golem_api_grpc::proto::golem::component::ComponentId {
                value: Some(value.component_id.component_id.0.into()),
            };
            Self {
                component_id: Some(component_id),
                component_version,
                reason: value.reason,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::worker::v1::GetLatestVersionOfComponentFailed>
        for GolemErrorGetLatestVersionOfComponentFailed
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::worker::v1::GetLatestVersionOfComponentFailed,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                component_id: value
                    .component_id
                    .ok_or("Missing field: component_id")?
                    .try_into()?,
                reason: value.reason,
            })
        }
    }

    impl From<GolemErrorGetLatestVersionOfComponentFailed>
        for golem_api_grpc::proto::golem::worker::v1::GetLatestVersionOfComponentFailed
    {
        fn from(value: GolemErrorGetLatestVersionOfComponentFailed) -> Self {
            Self {
                component_id: Some(value.component_id.into()),
                reason: value.reason,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::worker::v1::PromiseNotFound>
        for GolemErrorPromiseNotFound
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::worker::v1::PromiseNotFound,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                promise_id: value
                    .promise_id
                    .ok_or("Missing field: promise_id")?
                    .try_into()?,
            })
        }
    }

    impl From<GolemErrorPromiseNotFound> for golem_api_grpc::proto::golem::worker::v1::PromiseNotFound {
        fn from(value: GolemErrorPromiseNotFound) -> Self {
            Self {
                promise_id: Some(value.promise_id.into()),
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::worker::v1::PromiseDropped>
        for GolemErrorPromiseDropped
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::worker::v1::PromiseDropped,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                promise_id: value
                    .promise_id
                    .ok_or("Missing field: promise_id")?
                    .try_into()?,
            })
        }
    }

    impl From<GolemErrorPromiseDropped> for golem_api_grpc::proto::golem::worker::v1::PromiseDropped {
        fn from(value: GolemErrorPromiseDropped) -> Self {
            Self {
                promise_id: Some(value.promise_id.into()),
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::worker::v1::PromiseAlreadyCompleted>
        for GolemErrorPromiseAlreadyCompleted
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::worker::v1::PromiseAlreadyCompleted,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                promise_id: value
                    .promise_id
                    .ok_or("Missing field: promise_id")?
                    .try_into()?,
            })
        }
    }

    impl From<GolemErrorPromiseAlreadyCompleted>
        for golem_api_grpc::proto::golem::worker::v1::PromiseAlreadyCompleted
    {
        fn from(value: GolemErrorPromiseAlreadyCompleted) -> Self {
            Self {
                promise_id: Some(value.promise_id.into()),
            }
        }
    }

    impl From<golem_api_grpc::proto::golem::worker::v1::Interrupted> for GolemErrorInterrupted {
        fn from(value: golem_api_grpc::proto::golem::worker::v1::Interrupted) -> Self {
            Self {
                recover_immediately: value.recover_immediately,
            }
        }
    }

    impl From<GolemErrorInterrupted> for golem_api_grpc::proto::golem::worker::v1::Interrupted {
        fn from(value: GolemErrorInterrupted) -> Self {
            Self {
                recover_immediately: value.recover_immediately,
            }
        }
    }

    impl From<golem_api_grpc::proto::golem::worker::v1::ParamTypeMismatch>
        for GolemErrorParamTypeMismatch
    {
        fn from(value: golem_api_grpc::proto::golem::worker::v1::ParamTypeMismatch) -> Self {
            Self {
                details: value.details,
            }
        }
    }

    impl From<GolemErrorParamTypeMismatch>
        for golem_api_grpc::proto::golem::worker::v1::ParamTypeMismatch
    {
        fn from(value: GolemErrorParamTypeMismatch) -> Self {
            Self {
                details: value.details,
            }
        }
    }

    impl From<golem_api_grpc::proto::golem::worker::v1::NoValueInMessage>
        for GolemErrorNoValueInMessage
    {
        fn from(_value: golem_api_grpc::proto::golem::worker::v1::NoValueInMessage) -> Self {
            Self {}
        }
    }

    impl From<GolemErrorNoValueInMessage>
        for golem_api_grpc::proto::golem::worker::v1::NoValueInMessage
    {
        fn from(_value: GolemErrorNoValueInMessage) -> Self {
            Self {}
        }
    }

    impl From<golem_api_grpc::proto::golem::worker::v1::ValueMismatch> for GolemErrorValueMismatch {
        fn from(value: golem_api_grpc::proto::golem::worker::v1::ValueMismatch) -> Self {
            Self {
                details: value.details,
            }
        }
    }

    impl From<GolemErrorValueMismatch> for golem_api_grpc::proto::golem::worker::v1::ValueMismatch {
        fn from(value: GolemErrorValueMismatch) -> Self {
            Self {
                details: value.details,
            }
        }
    }

    impl From<golem_api_grpc::proto::golem::worker::v1::UnexpectedOplogEntry>
        for GolemErrorUnexpectedOplogEntry
    {
        fn from(value: golem_api_grpc::proto::golem::worker::v1::UnexpectedOplogEntry) -> Self {
            Self {
                expected: value.expected,
                got: value.got,
            }
        }
    }

    impl From<GolemErrorUnexpectedOplogEntry>
        for golem_api_grpc::proto::golem::worker::v1::UnexpectedOplogEntry
    {
        fn from(value: GolemErrorUnexpectedOplogEntry) -> Self {
            Self {
                expected: value.expected,
                got: value.got,
            }
        }
    }

    impl From<golem_api_grpc::proto::golem::worker::v1::RuntimeError> for GolemErrorRuntimeError {
        fn from(value: golem_api_grpc::proto::golem::worker::v1::RuntimeError) -> Self {
            Self {
                details: value.details,
            }
        }
    }

    impl From<GolemErrorRuntimeError> for golem_api_grpc::proto::golem::worker::v1::RuntimeError {
        fn from(value: GolemErrorRuntimeError) -> Self {
            Self {
                details: value.details,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::worker::v1::InvalidShardId>
        for GolemErrorInvalidShardId
    {
        type Error = String;
        fn try_from(
            value: golem_api_grpc::proto::golem::worker::v1::InvalidShardId,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                shard_id: value.shard_id.ok_or("Missing field: shard_id")?.into(),
                shard_ids: value.shard_ids.into_iter().map(|id| id.into()).collect(),
            })
        }
    }

    impl From<GolemErrorInvalidShardId> for golem_api_grpc::proto::golem::worker::v1::InvalidShardId {
        fn from(value: GolemErrorInvalidShardId) -> Self {
            Self {
                shard_id: Some(value.shard_id.into()),
                shard_ids: value.shard_ids.into_iter().map(|id| id.into()).collect(),
            }
        }
    }

    impl From<golem_api_grpc::proto::golem::worker::v1::PreviousInvocationFailed>
        for GolemErrorPreviousInvocationFailed
    {
        fn from(value: golem_api_grpc::proto::golem::worker::v1::PreviousInvocationFailed) -> Self {
            Self {
                details: value.details,
            }
        }
    }

    impl From<GolemErrorPreviousInvocationFailed>
        for golem_api_grpc::proto::golem::worker::v1::PreviousInvocationFailed
    {
        fn from(value: GolemErrorPreviousInvocationFailed) -> Self {
            Self {
                details: value.details,
            }
        }
    }

    impl From<golem_api_grpc::proto::golem::worker::v1::PreviousInvocationExited>
        for GolemErrorPreviousInvocationExited
    {
        fn from(
            _value: golem_api_grpc::proto::golem::worker::v1::PreviousInvocationExited,
        ) -> Self {
            Self {}
        }
    }

    impl From<GolemErrorPreviousInvocationExited>
        for golem_api_grpc::proto::golem::worker::v1::PreviousInvocationExited
    {
        fn from(_value: GolemErrorPreviousInvocationExited) -> Self {
            Self {}
        }
    }

    impl From<golem_api_grpc::proto::golem::worker::v1::UnknownError> for GolemErrorUnknown {
        fn from(value: golem_api_grpc::proto::golem::worker::v1::UnknownError) -> Self {
            Self {
                details: value.details,
            }
        }
    }

    impl From<GolemErrorUnknown> for golem_api_grpc::proto::golem::worker::v1::UnknownError {
        fn from(value: GolemErrorUnknown) -> Self {
            Self {
                details: value.details,
            }
        }
    }

    impl From<golem_api_grpc::proto::golem::worker::v1::InitialComponentFileDownloadFailed>
        for GolemErrorInitialComponentFileDownloadFailed
    {
        fn from(
            value: golem_api_grpc::proto::golem::worker::v1::InitialComponentFileDownloadFailed,
        ) -> Self {
            Self {
                path: value.path,
                reason: value.reason,
            }
        }
    }

    impl From<GolemErrorInitialComponentFileDownloadFailed>
        for golem_api_grpc::proto::golem::worker::v1::InitialComponentFileDownloadFailed
    {
        fn from(value: GolemErrorInitialComponentFileDownloadFailed) -> Self {
            Self {
                path: value.path,
                reason: value.reason,
            }
        }
    }

    impl From<golem_api_grpc::proto::golem::worker::v1::FileSystemError> for GolemErrorFileSystemError {
        fn from(value: golem_api_grpc::proto::golem::worker::v1::FileSystemError) -> Self {
            Self {
                path: value.path,
                reason: value.reason,
            }
        }
    }

    impl From<GolemErrorFileSystemError> for golem_api_grpc::proto::golem::worker::v1::FileSystemError {
        fn from(value: GolemErrorFileSystemError) -> Self {
            Self {
                path: value.path,
                reason: value.reason,
            }
        }
    }

    impl From<golem_api_grpc::proto::golem::worker::v1::ShardingNotReady>
        for GolemErrorShardingNotReady
    {
        fn from(_value: golem_api_grpc::proto::golem::worker::v1::ShardingNotReady) -> Self {
            Self {}
        }
    }

    impl From<GolemErrorShardingNotReady>
        for golem_api_grpc::proto::golem::worker::v1::ShardingNotReady
    {
        fn from(_value: GolemErrorShardingNotReady) -> Self {
            Self {}
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::worker::v1::WorkerExecutionError> for GolemError {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::worker::v1::WorkerExecutionError,
        ) -> Result<Self, Self::Error> {
            match value.error {
                Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::InvalidRequest(err)) => {
                    Ok(GolemError::InvalidRequest(err.into()))
                }
                Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::WorkerAlreadyExists(err)) => {
                    Ok(GolemError::WorkerAlreadyExists(err.try_into()?))
                }
                Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::WorkerNotFound(err)) => {
                    Ok(GolemError::WorkerNotFound(err.try_into()?))
                }
                Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::WorkerCreationFailed(err)) => {
                    Ok(GolemError::WorkerCreationFailed(err.try_into()?))
                }
                Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::FailedToResumeWorker(err)) => {
                    Ok(GolemError::FailedToResumeWorker((*err).try_into()?))
                }
                Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::ComponentDownloadFailed(err)) => {
                    Ok(GolemError::ComponentDownloadFailed(err.try_into()?))
                }
                Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::ComponentParseFailed(err)) => {
                    Ok(GolemError::ComponentParseFailed(err.try_into()?))
                }
                Some(
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::GetLatestVersionOfComponentFailed(
                        err,
                    ),
                ) => Ok(GolemError::GetLatestVersionOfComponentFailed(
                    err.try_into()?,
                )),
                Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::PromiseNotFound(err)) => {
                    Ok(GolemError::PromiseNotFound(err.try_into()?))
                }
                Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::PromiseDropped(err)) => {
                    Ok(GolemError::PromiseDropped(err.try_into()?))
                }
                Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::PromiseAlreadyCompleted(err)) => {
                    Ok(GolemError::PromiseAlreadyCompleted(err.try_into()?))
                }
                Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::Interrupted(err)) => {
                    Ok(GolemError::Interrupted(err.into()))
                }
                Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::ParamTypeMismatch(err)) => {
                    Ok(GolemError::ParamTypeMismatch(err.into()))
                }
                Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::NoValueInMessage(err)) => {
                    Ok(GolemError::NoValueInMessage(err.into()))
                }
                Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::ValueMismatch(err)) => {
                    Ok(GolemError::ValueMismatch(err.into()))
                }
                Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::UnexpectedOplogEntry(err)) => {
                    Ok(GolemError::UnexpectedOplogEntry(err.into()))
                }
                Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::RuntimeError(err)) => {
                    Ok(GolemError::RuntimeError(err.into()))
                }
                Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::InvalidShardId(err)) => {
                    Ok(GolemError::InvalidShardId(err.try_into()?))
                }
                Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::PreviousInvocationFailed(err)) => {
                    Ok(GolemError::PreviousInvocationFailed(err.into()))
                }
                Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::PreviousInvocationExited(err)) => {
                    Ok(GolemError::PreviousInvocationExited(err.into()))
                }
                Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::Unknown(err)) => {
                    Ok(GolemError::Unknown(err.into()))
                }
                Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::InvalidAccount(err)) => {
                    Ok(GolemError::InvalidAccount(err.into()))
                }
                Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::ShardingNotReady(err)) => {
                    Ok(GolemError::ShardingNotReady(err.into()))
                }
                Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::InitialComponentFileDownloadFailed(err)) => {
                    Ok(GolemError::InitialComponentFileDownloadFailed(err.into()))
                }
                Some(golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::FileSystemError(err)) => {
                    Ok(GolemError::FileSystemError(err.into()))
                }
                None => Err("Missing field: error".to_string()),
            }
        }
    }

    impl From<GolemError> for golem_api_grpc::proto::golem::worker::v1::WorkerExecutionError {
        fn from(error: GolemError) -> Self {
            golem_api_grpc::proto::golem::worker::v1::WorkerExecutionError {
                error: Some(error.into()),
            }
        }
    }

    impl From<GolemError> for golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error {
        fn from(error: GolemError) -> Self {
            match error {
                GolemError::InvalidRequest(err) => {
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::InvalidRequest(err.into())
                }
                GolemError::WorkerAlreadyExists(err) => {
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::WorkerAlreadyExists(err.into())
                }
                GolemError::WorkerNotFound(err) => {
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::WorkerNotFound(err.into())
                }
                GolemError::WorkerCreationFailed(err) => {
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::WorkerCreationFailed(err.into())
                }
                GolemError::FailedToResumeWorker(err) => {
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::FailedToResumeWorker(Box::new(err.into()))
                }
                GolemError::ComponentDownloadFailed(err) => {
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::ComponentDownloadFailed(err.into())
                }
                GolemError::ComponentParseFailed(err) => {
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::ComponentParseFailed(err.into())
                }
                GolemError::GetLatestVersionOfComponentFailed(err) => {
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::GetLatestVersionOfComponentFailed(err.into())
                }
                GolemError::PromiseNotFound(err) => {
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::PromiseNotFound(err.into())
                }
                GolemError::PromiseDropped(err) => {
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::PromiseDropped(err.into())
                }
                GolemError::PromiseAlreadyCompleted(err) => {
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::PromiseAlreadyCompleted(err.into())
                }
                GolemError::Interrupted(err) => {
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::Interrupted(err.into())
                }
                GolemError::ParamTypeMismatch(err) => {
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::ParamTypeMismatch(err.into())
                }
                GolemError::NoValueInMessage(err) => {
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::NoValueInMessage(err.into())
                }
                GolemError::ValueMismatch(err) => {
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::ValueMismatch(err.into())
                }
                GolemError::UnexpectedOplogEntry(err) => {
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::UnexpectedOplogEntry(err.into())
                }
                GolemError::RuntimeError(err) => {
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::RuntimeError(err.into())
                }
                GolemError::InvalidShardId(err) => {
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::InvalidShardId(err.into())
                }
                GolemError::PreviousInvocationFailed(err) => {
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::PreviousInvocationFailed(err.into())
                }
                GolemError::PreviousInvocationExited(err) => {
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::PreviousInvocationExited(err.into())
                }
                GolemError::Unknown(err) => {
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::Unknown(err.into())
                }
                GolemError::InvalidAccount(err) => {
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::InvalidAccount(err.into())
                }
                GolemError::ShardingNotReady(err) => {
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::ShardingNotReady(err.into())
                }
                GolemError::InitialComponentFileDownloadFailed(err) => {
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::InitialComponentFileDownloadFailed(err.into())
                }
                GolemError::FileSystemError(err) => {
                    golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error::FileSystemError(err.into())
                }
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::worker::v1::WorkerExecutionError> for GolemErrorBody {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::worker::v1::WorkerExecutionError,
        ) -> Result<Self, Self::Error> {
            Ok(Self {
                golem_error: value.try_into()?,
            })
        }
    }

    impl From<golem_api_grpc::proto::golem::common::ErrorBody> for ErrorBody {
        fn from(value: golem_api_grpc::proto::golem::common::ErrorBody) -> Self {
            Self { error: value.error }
        }
    }

    impl From<golem_api_grpc::proto::golem::common::ErrorsBody> for ErrorsBody {
        fn from(value: golem_api_grpc::proto::golem::common::ErrorsBody) -> Self {
            Self {
                errors: value.errors,
            }
        }
    }
}
