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

use golem_api_grpc::proto::golem::worker::v1::{
    worker_error, worker_execution_error, UnknownError, WorkerError as GrpcWorkerError,
};
use golem_common::model::component::VersionedComponentId;
use golem_common::model::error::GolemError;
use golem_common::model::{AccountId, ComponentFilePath, ComponentId, WorkerId};
use golem_common::SafeDisplay;

use crate::service::component::ComponentServiceError;
use crate::service::worker::CallWorkerExecutorError;

#[derive(Debug, thiserror::Error)]
pub enum WorkerServiceError {
    #[error(transparent)]
    Component(#[from] ComponentServiceError),
    #[error("Type checker error: {0}")]
    TypeChecker(String),
    #[error("Component not found: {0}")]
    VersionedComponentIdNotFound(VersionedComponentId),
    #[error("Component not found: {0}")]
    ComponentNotFound(ComponentId),
    #[error("Account not found: {0}")]
    AccountIdNotFound(AccountId),
    #[error("Worker not found: {0}")]
    WorkerNotFound(WorkerId),
    #[error("Internal error: {0}")]
    Internal(String),
    #[error(transparent)]
    Golem(GolemError),
    #[error(transparent)]
    InternalCallError(CallWorkerExecutorError),
    #[error("File not found: {0}")]
    FileNotFound(ComponentFilePath),
    #[error("Bad file type: {0}")]
    BadFileType(ComponentFilePath),
}

impl SafeDisplay for WorkerServiceError {
    fn to_safe_string(&self) -> String {
        match self {
            WorkerServiceError::Component(inner) => inner.to_safe_string(),
            WorkerServiceError::TypeChecker(_) => self.to_string(),
            WorkerServiceError::VersionedComponentIdNotFound(_) => self.to_string(),
            WorkerServiceError::ComponentNotFound(_) => self.to_string(),
            WorkerServiceError::AccountIdNotFound(_) => self.to_string(),
            WorkerServiceError::WorkerNotFound(_) => self.to_string(),
            WorkerServiceError::Internal(_) => self.to_string(),
            WorkerServiceError::Golem(inner) => inner.to_safe_string(),
            WorkerServiceError::InternalCallError(inner) => inner.to_safe_string(),
            WorkerServiceError::FileNotFound(_) => self.to_string(),
            WorkerServiceError::BadFileType(_) => self.to_string(),
        }
    }
}

impl From<WorkerServiceError> for GrpcWorkerError {
    fn from(error: WorkerServiceError) -> Self {
        GrpcWorkerError {
            error: Some(error.into()),
        }
    }
}

impl From<WorkerServiceError> for worker_error::Error {
    fn from(error: WorkerServiceError) -> Self {
        use golem_api_grpc::proto::golem::common::{ErrorBody, ErrorsBody};
        use golem_api_grpc::proto::golem::worker::v1::WorkerExecutionError;

        match error {
            error @ (WorkerServiceError::ComponentNotFound(_)
            | WorkerServiceError::AccountIdNotFound(_)
            | WorkerServiceError::VersionedComponentIdNotFound(_)
            | WorkerServiceError::WorkerNotFound(_)) => worker_error::Error::NotFound(ErrorBody {
                error: error.to_safe_string(),
            }),
            WorkerServiceError::Internal(_) => {
                worker_error::Error::InternalError(WorkerExecutionError {
                    error: Some(worker_execution_error::Error::Unknown(UnknownError {
                        details: error.to_safe_string(),
                    })),
                })
            }
            WorkerServiceError::InternalCallError(_) => {
                worker_error::Error::InternalError(WorkerExecutionError {
                    error: Some(worker_execution_error::Error::Unknown(UnknownError {
                        details: error.to_safe_string(),
                    })),
                })
            }
            WorkerServiceError::TypeChecker(error) => worker_error::Error::BadRequest(ErrorsBody {
                errors: vec![error],
            }),
            WorkerServiceError::Component(component) => component.into(),
            WorkerServiceError::Golem(worker_execution_error) => {
                worker_error::Error::InternalError(worker_execution_error.into())
            }
            WorkerServiceError::FileNotFound(_) => worker_error::Error::NotFound(ErrorBody {
                error: error.to_safe_string(),
            }),
            WorkerServiceError::BadFileType(_) => worker_error::Error::BadRequest(ErrorsBody {
                errors: vec![error.to_safe_string()],
            }),
        }
    }
}

impl From<GolemError> for WorkerServiceError {
    fn from(value: GolemError) -> Self {
        WorkerServiceError::Golem(value)
    }
}
