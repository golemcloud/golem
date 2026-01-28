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

use crate::service::auth::AuthServiceError;
use crate::service::component::ComponentServiceError;
use crate::service::limit::LimitServiceError;
use crate::service::worker::CallWorkerExecutorError;
use golem_common::SafeDisplay;
use golem_common::model::WorkerId;
use golem_common::model::account::AccountId;
use golem_common::model::component::{ComponentFilePath, ComponentId};
use golem_service_base::clients::registry::RegistryServiceError;
use golem_service_base::error::worker_executor::WorkerExecutorError;

#[derive(Debug, thiserror::Error)]
pub enum WorkerServiceError {
    #[error(transparent)]
    Component(#[from] ComponentServiceError),
    #[error(transparent)]
    LimitError(#[from] LimitServiceError),
    #[error(transparent)]
    AuthError(#[from] AuthServiceError),
    #[error(transparent)]
    InternalCallError(CallWorkerExecutorError),
    #[error(transparent)]
    GolemError(#[from] WorkerExecutorError),
    #[error("Type checker error: {0}")]
    TypeChecker(String),
    #[error("Component not found: {0}")]
    ComponentNotFound(ComponentId),
    #[error("Account not found: {0}")]
    AccountIdNotFound(AccountId),
    #[error("Worker not found: {0}")]
    WorkerNotFound(WorkerId),
    #[error("Internal error: {0}")]
    Internal(String),
    #[error("File not found: {0}")]
    FileNotFound(ComponentFilePath),
    #[error("Bad file type: {0}")]
    BadFileType(ComponentFilePath),
    #[error(transparent)]
    RegistryServiceError(#[from] RegistryServiceError),
}

impl SafeDisplay for WorkerServiceError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::Component(inner) => inner.to_safe_string(),
            Self::TypeChecker(_) => self.to_string(),
            Self::ComponentNotFound(_) => self.to_string(),
            Self::AccountIdNotFound(_) => self.to_string(),
            Self::WorkerNotFound(_) => self.to_string(),
            Self::Internal(_) => self.to_string(),
            Self::GolemError(inner) => inner.to_safe_string(),
            Self::InternalCallError(inner) => inner.to_safe_string(),
            Self::FileNotFound(_) => self.to_string(),
            Self::BadFileType(_) => self.to_string(),
            Self::LimitError(inner) => inner.to_safe_string(),
            Self::AuthError(inner) => inner.to_safe_string(),
            Self::RegistryServiceError(inner) => inner.to_safe_string(),
        }
    }
}

impl From<WorkerServiceError> for golem_api_grpc::proto::golem::worker::v1::WorkerError {
    fn from(error: WorkerServiceError) -> Self {
        Self {
            error: Some(error.into()),
        }
    }
}

impl From<WorkerServiceError> for golem_api_grpc::proto::golem::worker::v1::worker_error::Error {
    fn from(error: WorkerServiceError) -> Self {
        use golem_api_grpc::proto::golem::common::{ErrorBody, ErrorsBody};
        use golem_api_grpc::proto::golem::worker::v1::UnknownError;
        use golem_api_grpc::proto::golem::worker::v1::WorkerExecutionError;
        use golem_api_grpc::proto::golem::worker::v1::worker_execution_error::Error as GrpcError;

        match error {
            WorkerServiceError::ComponentNotFound(_)
            | WorkerServiceError::AccountIdNotFound(_)
            | WorkerServiceError::WorkerNotFound(_)
            | WorkerServiceError::FileNotFound(_)
            | WorkerServiceError::RegistryServiceError(RegistryServiceError::NotFound(_))
            | WorkerServiceError::Component(ComponentServiceError::ComponentNotFound)
            | WorkerServiceError::GolemError(WorkerExecutorError::WorkerNotFound { .. }) => {
                Self::NotFound(ErrorBody {
                    error: error.to_safe_string(),
                })
            }

            WorkerServiceError::BadFileType(_)
            | WorkerServiceError::TypeChecker(_)
            | WorkerServiceError::RegistryServiceError(RegistryServiceError::BadRequest(_)) => {
                Self::BadRequest(ErrorsBody {
                    errors: vec![error.to_safe_string()],
                })
            }

            WorkerServiceError::LimitError(LimitServiceError::LimitExceeded(_))
            | WorkerServiceError::RegistryServiceError(RegistryServiceError::LimitExceeded(_)) => {
                Self::LimitExceeded(ErrorBody {
                    error: error.to_safe_string(),
                })
            }

            WorkerServiceError::Internal(_)
            | WorkerServiceError::RegistryServiceError(_)
            | WorkerServiceError::InternalCallError(_)
            | WorkerServiceError::LimitError(_)
            | WorkerServiceError::AuthError(_)
            | WorkerServiceError::Component(ComponentServiceError::InternalError(_)) => {
                Self::InternalError(WorkerExecutionError {
                    error: Some(GrpcError::Unknown(UnknownError {
                        details: error.to_safe_string(),
                    })),
                })
            }

            WorkerServiceError::GolemError(worker_execution_error) => {
                Self::InternalError(worker_execution_error.into())
            }
        }
    }
}
