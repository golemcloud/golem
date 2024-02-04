use golem_client::api::{TemplateError, WorkerError};
use golem_client::model::{
    GolemError, GolemErrorFailedToResumeWorker, GolemErrorGetLatestVersionOfTemplateFailed,
    GolemErrorInterrupted, GolemErrorInvalidRequest, GolemErrorInvalidShardId,
    GolemErrorPromiseAlreadyCompleted, GolemErrorPromiseDropped, GolemErrorPromiseNotFound,
    GolemErrorRuntimeError, GolemErrorTemplateDownloadFailed, GolemErrorTemplateParseFailed,
    GolemErrorUnexpectedOplogEntry, GolemErrorUnknown, GolemErrorValueMismatch,
    GolemErrorWorkerAlreadyExists, GolemErrorWorkerCreationFailed, GolemErrorWorkerNotFound,
    PromiseId, WorkerId,
};
use itertools::Itertools;

pub trait ResponseContentErrorMapper {
    fn map(self) -> String;
}

impl ResponseContentErrorMapper for TemplateError {
    fn map(self) -> String {
        match self {
            TemplateError::Error400(errors) => errors.errors.iter().join(", "),
            TemplateError::Error401(error) => error.error,
            TemplateError::Error403(error) => error.error,
            TemplateError::Error404(error) => error.error,
            TemplateError::Error409(error) => error.error,
            TemplateError::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for WorkerError {
    fn map(self) -> String {
        match self {
            WorkerError::Error400(errors) => errors.errors.iter().join(", "),
            WorkerError::Error404(error) => error.error,
            WorkerError::Error409(error) => error.error,
            WorkerError::Error500(error) => display_golem_error(error.golem_error),
        }
    }
}

fn display_golem_error(error: golem_client::model::GolemError) -> String {
    match error {
        GolemError::InvalidRequest(GolemErrorInvalidRequest { details }) => {
            format!("Invalid request: {details}")
        }
        GolemError::WorkerAlreadyExists(GolemErrorWorkerAlreadyExists { worker_id }) => {
            format!("Worker already exists: {}", display_worker_id(worker_id))
        }
        GolemError::WorkerNotFound(GolemErrorWorkerNotFound { worker_id }) => {
            format!("Worker not found: {}", display_worker_id(worker_id))
        }
        GolemError::WorkerCreationFailed(GolemErrorWorkerCreationFailed { worker_id, details }) => {
            format!(
                "Failed to create worker {}: {}",
                display_worker_id(worker_id),
                details
            )
        }
        GolemError::FailedToResumeWorker(GolemErrorFailedToResumeWorker { worker_id }) => {
            format!("Failed to resume worker: {}", display_worker_id(worker_id))
        }
        GolemError::TemplateDownloadFailed(GolemErrorTemplateDownloadFailed {
            template_id,
            reason,
        }) => {
            format!(
                "Failed to download template {}#{}: {}",
                template_id.template_id, template_id.version, reason
            )
        }
        GolemError::TemplateParseFailed(GolemErrorTemplateParseFailed {
            template_id,
            reason,
        }) => {
            format!(
                "Failed to parse template {}#{}: {}",
                template_id.template_id, template_id.version, reason
            )
        }
        GolemError::GetLatestVersionOfTemplateFailed(
            GolemErrorGetLatestVersionOfTemplateFailed {
                template_id,
                reason,
            },
        ) => {
            format!(
                "Failed to get latest version of template {}: {}",
                template_id, reason
            )
        }
        GolemError::PromiseNotFound(GolemErrorPromiseNotFound { promise_id }) => {
            format!("Promise not found: {}", display_promise_id(promise_id))
        }
        GolemError::PromiseDropped(GolemErrorPromiseDropped { promise_id }) => {
            format!("Promise dropped: {}", display_promise_id(promise_id))
        }
        GolemError::PromiseAlreadyCompleted(GolemErrorPromiseAlreadyCompleted { promise_id }) => {
            format!(
                "Promise already completed: {}",
                display_promise_id(promise_id)
            )
        }
        GolemError::Interrupted(GolemErrorInterrupted {
            recover_immediately,
        }) => {
            if recover_immediately {
                "Simulated crash".to_string()
            } else {
                "Worker interrupted".to_string()
            }
        }
        GolemError::ParamTypeMismatch(_) => "Parameter type mismatch".to_string(),
        GolemError::NoValueInMessage(_) => "No value in message".to_string(),
        GolemError::ValueMismatch(GolemErrorValueMismatch { details }) => {
            format!("Parameter value mismatch: {}", details)
        }
        GolemError::UnexpectedOplogEntry(GolemErrorUnexpectedOplogEntry { expected, got }) => {
            format!("Unexpected oplog entry: expected {}, got {}", expected, got)
        }
        GolemError::RuntimeError(GolemErrorRuntimeError { details }) => {
            format!("Runtime error: {}", details)
        }
        GolemError::InvalidShardId(GolemErrorInvalidShardId {
            shard_id,
            shard_ids,
        }) => {
            format!(
                "Invalid shard id: {} not in [{}]",
                shard_id.value,
                shard_ids.iter().map(|id| id.value).join(", ")
            )
        }
        GolemError::PreviousInvocationFailed(_) => {
            "The previously invoked function failed".to_string()
        }
        GolemError::PreviousInvocationExited(_) => {
            "The previously invoked function exited".to_string()
        }
        GolemError::Unknown(GolemErrorUnknown { details }) => {
            format!("Unknown error: {}", details)
        }
        GolemError::InvalidAccount(_) => "Invalid account".to_string(),
    }
}

fn display_worker_id(worker_id: WorkerId) -> String {
    format!("{}/{}", worker_id.template_id, worker_id.worker_name)
}

fn display_promise_id(promise_id: PromiseId) -> String {
    format!(
        "{}/{}",
        display_worker_id(promise_id.worker_id),
        promise_id.oplog_idx
    )
}
