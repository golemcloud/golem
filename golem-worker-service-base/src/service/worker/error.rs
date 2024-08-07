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

use golem_api_grpc::proto::golem::worker::v1::{
    worker_error, worker_execution_error, UnknownError, WorkerError as GrpcWorkerError,
};
use golem_common::model::{AccountId, ComponentId, WorkerId};
use golem_service_base::model::{GolemError, VersionedComponentId};

use crate::service::component::ComponentServiceError;

#[derive(Debug, thiserror::Error)]
pub enum WorkerServiceError {
    #[error(transparent)]
    Component(#[from] ComponentServiceError),
    // TODO: This should prob be a vec?
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
    Internal(#[from] anyhow::Error),
    #[error(transparent)]
    Golem(GolemError),
}

impl WorkerServiceError {
    pub fn internal<M>(error: M) -> Self
    where
        M: std::error::Error + Send + Sync + 'static,
    {
        Self::Internal(anyhow::Error::new(error))
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
                error: error.to_string(),
            }),
            WorkerServiceError::Internal(_) => {
                worker_error::Error::InternalError(WorkerExecutionError {
                    error: Some(worker_execution_error::Error::Unknown(UnknownError {
                        details: error.to_string(),
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
        }
    }
}

impl From<GolemError> for WorkerServiceError {
    fn from(value: GolemError) -> Self {
        WorkerServiceError::Golem(value)
    }
}
