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

use tonic::Status;

use golem_api_grpc::proto::golem::worker::v1::{
    worker_error, worker_execution_error, UnknownError, WorkerError as GrpcWorkerError,
    WorkerExecutionError,
};
use golem_common::SafeDisplay;

// The dependents of golem-worker-service-base is expected
// to have a component service internally that can depend on this base error
#[derive(Debug, thiserror::Error)]
pub enum ComponentServiceError {
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Forbidden: {0}")]
    Forbidden(String),
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Bad Request: {}", .0.join(", "))]
    BadRequest(Vec<String>),
    #[error("Already Exists: {0}")]
    AlreadyExists(String),
    #[error("Internal component service error: {0}")]
    Internal(String),
    #[error("Internal error: {0}")]
    FailedGrpcStatus(Status),
    #[error("Internal error: {0}")]
    FailedTransport(tonic::transport::Error),
}

impl SafeDisplay for ComponentServiceError {
    fn to_safe_string(&self) -> String {
        match self {
            ComponentServiceError::Unauthorized(_) => self.to_string(),
            ComponentServiceError::Forbidden(_) => self.to_string(),
            ComponentServiceError::NotFound(_) => self.to_string(),
            ComponentServiceError::BadRequest(_) => self.to_string(),
            ComponentServiceError::AlreadyExists(_) => self.to_string(),
            ComponentServiceError::Internal(_) => self.to_string(),
            ComponentServiceError::FailedGrpcStatus(_) => self.to_string(),
            ComponentServiceError::FailedTransport(_) => self.to_string(),
        }
    }
}

impl From<Status> for ComponentServiceError {
    fn from(status: Status) -> Self {
        ComponentServiceError::FailedGrpcStatus(status)
    }
}

impl From<tonic::transport::Error> for ComponentServiceError {
    fn from(error: tonic::transport::Error) -> Self {
        ComponentServiceError::FailedTransport(error)
    }
}

impl From<golem_api_grpc::proto::golem::component::v1::ComponentError> for ComponentServiceError {
    fn from(error: golem_api_grpc::proto::golem::component::v1::ComponentError) -> Self {
        use golem_api_grpc::proto::golem::component::v1::component_error::Error;
        match error.error {
            Some(Error::BadRequest(errors)) => ComponentServiceError::BadRequest(errors.errors),
            Some(Error::Unauthorized(error)) => ComponentServiceError::Unauthorized(error.error),
            Some(Error::LimitExceeded(error)) => ComponentServiceError::Forbidden(error.error),
            Some(Error::NotFound(error)) => ComponentServiceError::NotFound(error.error),
            Some(Error::AlreadyExists(error)) => ComponentServiceError::AlreadyExists(error.error),
            Some(Error::InternalError(error)) => ComponentServiceError::Internal(error.error),
            None => ComponentServiceError::Internal("Unknown error".to_string()),
        }
    }
}

impl From<ComponentServiceError> for GrpcWorkerError {
    fn from(error: ComponentServiceError) -> Self {
        GrpcWorkerError {
            error: Some(error.into()),
        }
    }
}

impl From<ComponentServiceError> for worker_error::Error {
    fn from(value: ComponentServiceError) -> Self {
        use golem_api_grpc::proto::golem::common::{ErrorBody, ErrorsBody};

        match value {
            ComponentServiceError::Unauthorized(error) => {
                worker_error::Error::Unauthorized(ErrorBody { error })
            }
            ComponentServiceError::Forbidden(error) => {
                worker_error::Error::LimitExceeded(ErrorBody { error })
            }
            ComponentServiceError::NotFound(error) => {
                worker_error::Error::NotFound(ErrorBody { error })
            }
            ComponentServiceError::AlreadyExists(error) => {
                worker_error::Error::AlreadyExists(ErrorBody { error })
            }
            ComponentServiceError::BadRequest(errors) => {
                worker_error::Error::BadRequest(ErrorsBody { errors })
            }
            ComponentServiceError::Internal(error) => {
                worker_error::Error::InternalError(WorkerExecutionError {
                    error: Some(worker_execution_error::Error::Unknown(UnknownError {
                        details: error.to_string(),
                    })),
                })
            }
            ComponentServiceError::FailedGrpcStatus(status) => {
                worker_error::Error::InternalError(WorkerExecutionError {
                    error: Some(worker_execution_error::Error::Unknown(UnknownError {
                        details: status.to_string(),
                    })),
                })
            }
            ComponentServiceError::FailedTransport(error) => {
                worker_error::Error::InternalError(WorkerExecutionError {
                    error: Some(worker_execution_error::Error::Unknown(UnknownError {
                        details: error.to_string(),
                    })),
                })
            }
        }
    }
}
