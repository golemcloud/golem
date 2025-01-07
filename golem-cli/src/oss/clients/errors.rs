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

use crate::model::ResponseContentErrorMapper;
use golem_client::api::{
    ApiDefinitionError, ApiDeploymentError, ApiSecurityError, ComponentError, HealthCheckError,
    PluginError, WorkerError,
};
use golem_client::model::{
    GolemError, GolemErrorComponentDownloadFailed, GolemErrorComponentParseFailed,
    GolemErrorFailedToResumeWorker, GolemErrorFileSystemError,
    GolemErrorGetLatestVersionOfComponentFailed, GolemErrorInitialComponentFileDownloadFailed,
    GolemErrorInterrupted, GolemErrorInvalidRequest, GolemErrorInvalidShardId,
    GolemErrorPromiseAlreadyCompleted, GolemErrorPromiseDropped, GolemErrorPromiseNotFound,
    GolemErrorRuntimeError, GolemErrorUnexpectedOplogEntry, GolemErrorUnknown,
    GolemErrorValueMismatch, GolemErrorWorkerAlreadyExists, GolemErrorWorkerCreationFailed,
    GolemErrorWorkerNotFound, PromiseId, WorkerId, WorkerServiceErrorsBody,
};
use itertools::Itertools;

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

impl ResponseContentErrorMapper for PluginError {
    fn map(self) -> String {
        match self {
            PluginError::Error400(errors) => errors.errors.iter().join(", "),
            PluginError::Error401(error) => error.error,
            PluginError::Error403(error) => error.error,
            PluginError::Error404(error) => error.error,
            PluginError::Error409(error) => error.error,
            PluginError::Error500(error) => error.error,
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
        "Invalid request".to_string()
    }
}

impl ResponseContentErrorMapper for ApiDefinitionError {
    fn map(self) -> String {
        match self {
            ApiDefinitionError::Error400(error) => display_worker_service_errors_body(error),
            ApiDefinitionError::Error401(error) => error.error,
            ApiDefinitionError::Error403(error) => error.error,
            ApiDefinitionError::Error404(error) => error.error,
            ApiDefinitionError::Error409(error) => error.to_string(),
            ApiDefinitionError::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for ApiSecurityError {
    fn map(self) -> String {
        match self {
            ApiSecurityError::Error400(error) => display_worker_service_errors_body(error),
            ApiSecurityError::Error401(error) => error.error,
            ApiSecurityError::Error403(error) => error.error,
            ApiSecurityError::Error404(error) => error.error,
            ApiSecurityError::Error409(error) => error.to_string(),
            ApiSecurityError::Error500(error) => error.error,
        }
    }
}

impl ResponseContentErrorMapper for ApiDeploymentError {
    fn map(self) -> String {
        match self {
            ApiDeploymentError::Error400(error) => display_worker_service_errors_body(error),
            ApiDeploymentError::Error401(error) => error.error,
            ApiDeploymentError::Error403(error) => error.error,
            ApiDeploymentError::Error404(error) => error.error,
            ApiDeploymentError::Error409(error) => error.to_string(),
            ApiDeploymentError::Error500(error) => error.error,
        }
    }
}

pub fn display_golem_error(error: GolemError) -> String {
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
                shard_id,
                shard_ids.iter().join(", ")
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
        GolemError::ShardingNotReady(_) => "Sharding not ready".to_string(),
        GolemError::InitialComponentFileDownloadFailed(
            GolemErrorInitialComponentFileDownloadFailed { path, reason, .. },
        ) => {
            format!("Failed to download initial file {}: {}", path, reason)
        }
        GolemError::FileSystemError(GolemErrorFileSystemError { path, reason, .. }) => {
            format!("Error working with file {}: {}", path, reason)
        }
    }
}

pub fn display_worker_id(worker_id: WorkerId) -> String {
    format!("{}/{}", worker_id.component_id, worker_id.worker_name)
}

pub fn display_promise_id(promise_id: PromiseId) -> String {
    format!(
        "{}/{}",
        display_worker_id(promise_id.worker_id),
        promise_id.oplog_idx
    )
}

pub fn display_worker_service_errors_body(error: WorkerServiceErrorsBody) -> String {
    match error {
        WorkerServiceErrorsBody::Messages(messages) => messages.errors.iter().join(", "),
        WorkerServiceErrorsBody::Validation(validation) => validation.errors.iter().join("\n"),
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::oss::clients::errors::ResponseContentErrorMapper;
    use golem_client::{
        api::ApiDefinitionError,
        model::{ErrorBody, MessagesErrorsBody, ValidationErrorsBody, WorkerServiceErrorsBody},
    };

    #[test]
    fn api_definition_error_409() {
        let error = ApiDefinitionError::Error409("409".to_string());
        assert_eq!(error.map(), "409".to_string())
    }

    #[test]
    fn api_definition_error_401() {
        let error = ApiDefinitionError::Error401(ErrorBody {
            error: "401".to_string(),
        });
        assert_eq!(error.map(), "401".to_string())
    }

    #[test]
    fn api_definition_error_403() {
        let error = ApiDefinitionError::Error403(ErrorBody {
            error: "403".to_string(),
        });
        assert_eq!(error.map(), "403".to_string())
    }

    #[test]
    fn api_definition_error_404() {
        let error = ApiDefinitionError::Error404(ErrorBody {
            error: "404".to_string(),
        });
        assert_eq!(error.map(), "404".to_string())
    }

    #[test]
    fn api_definition_error_500() {
        let error = ApiDefinitionError::Error500(ErrorBody {
            error: "500".to_string(),
        });
        assert_eq!(error.map(), "500".to_string())
    }

    #[test]
    fn api_definition_error_400_messages() {
        let error =
            ApiDefinitionError::Error400(WorkerServiceErrorsBody::Messages(MessagesErrorsBody {
                errors: vec!["400_1".to_string(), "400_2".to_string()],
            }));
        assert_eq!(error.map(), "400_1, 400_2".to_string())
    }

    #[test]
    fn api_definition_error_400_validation() {
        let error = ApiDefinitionError::Error400(WorkerServiceErrorsBody::Validation(
            ValidationErrorsBody {
                errors: vec![
                    "Get/path/02f09a3f-1624-3b1d-8409-44eff7708208/Duplicate route".to_string(),
                    "Post/path2/02f09a3f-1624-3b1d-8409-44eff7708209/Other route".to_string(),
                ],
            },
        ));
        assert_eq!(error.map(), "Get/path/02f09a3f-1624-3b1d-8409-44eff7708208/Duplicate route\nPost/path2/02f09a3f-1624-3b1d-8409-44eff7708209/Other route".to_string())
    }
}
