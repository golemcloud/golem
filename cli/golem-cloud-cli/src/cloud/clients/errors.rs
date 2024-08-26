use golem_cloud_client::api::WorkerError;
use golem_cloud_client::api::{
    AccountError, ApiCertificateError, ApiDefinitionError, ApiDeploymentError, ApiDomainError,
    ComponentError, GrantError, HealthCheckError, LoginError, ProjectError, ProjectGrantError,
    ProjectPolicyError, TokenError,
};
use golem_cloud_client::model::{
    GolemError, GolemErrorComponentDownloadFailed, GolemErrorComponentParseFailed,
    GolemErrorFailedToResumeWorker, GolemErrorGetLatestVersionOfComponentFailed,
    GolemErrorInterrupted, GolemErrorInvalidRequest, GolemErrorInvalidShardId,
    GolemErrorPromiseAlreadyCompleted, GolemErrorPromiseDropped, GolemErrorPromiseNotFound,
    GolemErrorRuntimeError, GolemErrorUnexpectedOplogEntry, GolemErrorUnknown,
    GolemErrorValueMismatch, GolemErrorWorkerAlreadyExists, GolemErrorWorkerCreationFailed,
    GolemErrorWorkerNotFound, PromiseId, WorkerId, WorkerServiceErrorsBody,
};
use itertools::Itertools;

#[derive(Clone, PartialEq, Eq)]
pub struct CloudGolemError(pub String);

impl From<reqwest::Error> for CloudGolemError {
    fn from(error: reqwest::Error) -> Self {
        CloudGolemError(format!("Unexpected client error: {error:?}"))
    }
}

impl From<reqwest::header::InvalidHeaderValue> for CloudGolemError {
    fn from(value: reqwest::header::InvalidHeaderValue) -> Self {
        CloudGolemError(format!("Invalid request header: {value}"))
    }
}

impl From<CloudGolemError> for golem_cli::model::GolemError {
    fn from(value: CloudGolemError) -> Self {
        golem_cli::model::GolemError(value.0)
    }
}

pub trait ResponseContentErrorMapper {
    fn map(self) -> String;
}

impl<T: ResponseContentErrorMapper> From<golem_cloud_client::Error<T>> for CloudGolemError {
    fn from(value: golem_cloud_client::Error<T>) -> Self {
        match value {
            golem_cloud_client::Error::Reqwest(error) => CloudGolemError::from(error),
            golem_cloud_client::Error::ReqwestHeader(invalid_header) => {
                CloudGolemError::from(invalid_header)
            }
            golem_cloud_client::Error::Serde(error) => {
                CloudGolemError(format!("Unexpected serialization error: {error}"))
            }
            golem_cloud_client::Error::Item(data) => {
                let error_str = ResponseContentErrorMapper::map(data);
                CloudGolemError(error_str)
            }
            golem_cloud_client::Error::Unexpected { code, data } => {
                match String::from_utf8(Vec::from(data)) {
                    Ok(data_string) => CloudGolemError(format!(
                        "Unexpected http error. Code: {code}, content: {data_string}."
                    )),
                    Err(_) => CloudGolemError(format!(
                        "Unexpected http error. Code: {code}, can't parse content as string."
                    )),
                }
            }
        }
    }
}

impl ResponseContentErrorMapper for AccountError {
    fn map(self) -> String {
        match self {
            AccountError::Error400(errors) => errors.errors.iter().join(", "),
            AccountError::Error401(error) => error.error,
            AccountError::Error404(error) => error.error,
            AccountError::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for GrantError {
    fn map(self) -> String {
        match self {
            GrantError::Error400(errors) => errors.errors.iter().join(", "),
            GrantError::Error401(error) => error.error,
            GrantError::Error404(error) => error.error,
            GrantError::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for LoginError {
    fn map(self) -> String {
        match self {
            LoginError::Error400(errors) => errors.errors.iter().join(", "),
            LoginError::Error401(error) => error.error,
            LoginError::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for ProjectError {
    fn map(self) -> String {
        match self {
            ProjectError::Error400(errors) => errors.errors.iter().join(", "),
            ProjectError::Error401(error) => error.error,
            ProjectError::Error403(error) => error.error,
            ProjectError::Error404(error) => error.error,
            ProjectError::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for ProjectGrantError {
    fn map(self) -> String {
        match self {
            ProjectGrantError::Error400(errors) => errors.errors.iter().join(", "),
            ProjectGrantError::Error401(error) => error.error,
            ProjectGrantError::Error403(error) => error.error,
            ProjectGrantError::Error404(error) => error.error,
            ProjectGrantError::Error500(error) => error.error,
        }
    }
}

#[allow(unreachable_patterns)]
impl ResponseContentErrorMapper for ProjectPolicyError {
    fn map(self) -> String {
        match self {
            ProjectPolicyError::Error400(errors) => errors.errors.iter().join(", "),
            ProjectPolicyError::Error401(error) => error.error,
            ProjectPolicyError::Error404(error) => error.error,
            ProjectPolicyError::Error500(error) => error.error,
            _ => "UnknownError".into(),
        }
    }
}

impl ResponseContentErrorMapper for ComponentError {
    fn map(self) -> String {
        match self {
            ComponentError::Error400(errors) => errors.errors.iter().join(", "),
            ComponentError::Error401(error) => error.error,
            ComponentError::Error403(error) => error.error,
            ComponentError::Error404(error) => error.error,
            ComponentError::Error409(error) => error.error,
            ComponentError::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for TokenError {
    fn map(self) -> String {
        match self {
            TokenError::Error400(errors) => errors.errors.iter().join(", "),
            TokenError::Error401(error) => error.error,
            TokenError::Error404(error) => error.error,
            TokenError::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for WorkerError {
    fn map(self) -> String {
        match self {
            WorkerError::Error400(errors) => errors.errors.iter().join(", "),
            WorkerError::Error401(error) => error.error,
            WorkerError::Error403(error) => error.error,
            WorkerError::Error404(error) => error.error,
            WorkerError::Error409(error) => error.error,
            WorkerError::Error500(error) => display_golem_error(error.golem_error),
        }
    }
}

impl ResponseContentErrorMapper for HealthCheckError {
    fn map(self) -> String {
        match self {}
    }
}

impl ResponseContentErrorMapper for ApiCertificateError {
    fn map(self) -> String {
        match self {
            ApiCertificateError::Error400(errors) => display_worker_service_errors_body(errors),
            ApiCertificateError::Error401(error) => error.error,
            ApiCertificateError::Error403(error) => error.error,
            ApiCertificateError::Error404(message) => message.message,
            ApiCertificateError::Error409(error) => error.to_string(),
            ApiCertificateError::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for ApiDefinitionError {
    fn map(self) -> String {
        match self {
            ApiDefinitionError::Error400(errors) => display_worker_service_errors_body(errors),
            ApiDefinitionError::Error401(error) => error.error,
            ApiDefinitionError::Error403(error) => error.error,
            ApiDefinitionError::Error404(message) => message.message,
            ApiDefinitionError::Error409(error) => error.to_string(),
            ApiDefinitionError::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for ApiDeploymentError {
    fn map(self) -> String {
        match self {
            ApiDeploymentError::Error400(errors) => display_worker_service_errors_body(errors),
            ApiDeploymentError::Error401(error) => error.error,
            ApiDeploymentError::Error403(error) => error.error,
            ApiDeploymentError::Error404(message) => message.message,
            ApiDeploymentError::Error409(error) => error.to_string(),
            ApiDeploymentError::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for ApiDomainError {
    fn map(self) -> String {
        match self {
            ApiDomainError::Error400(errors) => display_worker_service_errors_body(errors),
            ApiDomainError::Error401(error) => error.error,
            ApiDomainError::Error403(error) => error.error,
            ApiDomainError::Error404(message) => message.message,
            ApiDomainError::Error409(error) => error.to_string(),
            ApiDomainError::Error500(error) => error.error,
        }
    }
}

fn display_golem_error(error: GolemError) -> String {
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
        GolemError::FailedToResumeWorker(inner) => {
            let GolemErrorFailedToResumeWorker { worker_id, reason } = *inner;
            format!(
                "Failed to resume worker {}: {}",
                display_worker_id(worker_id),
                display_golem_error(reason)
            )
        }
        GolemError::ComponentDownloadFailed(GolemErrorComponentDownloadFailed {
            component_id,
            reason,
        }) => {
            format!(
                "Failed to download component {}#{}: {}",
                component_id.component_id, component_id.version, reason
            )
        }
        GolemError::ComponentParseFailed(GolemErrorComponentParseFailed {
            component_id,
            reason,
        }) => {
            format!(
                "Failed to parse component {}#{}: {}",
                component_id.component_id, component_id.version, reason
            )
        }
        GolemError::GetLatestVersionOfComponentFailed(
            GolemErrorGetLatestVersionOfComponentFailed {
                component_id,
                reason,
            },
        ) => {
            format!(
                "Failed to get latest version of component {}: {}",
                component_id, reason
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
    format!("{}/{}", worker_id.component_id, worker_id.worker_name)
}

fn display_promise_id(promise_id: PromiseId) -> String {
    format!(
        "{}/{}",
        display_worker_id(promise_id.worker_id),
        promise_id.oplog_idx
    )
}

fn display_worker_service_errors_body(error: WorkerServiceErrorsBody) -> String {
    match error {
        WorkerServiceErrorsBody::Messages(messages) => messages.errors.iter().join(", "),
        WorkerServiceErrorsBody::Validation(validation) => validation
            .errors
            .iter()
            .map(|e| {
                format!(
                    "{}/{}/{}/{}",
                    e.method, e.path, e.component.component_id, e.detail
                )
            })
            .join("\n"),
    }
}
