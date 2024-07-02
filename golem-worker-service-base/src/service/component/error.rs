use tonic::Status;

use golem_api_grpc::proto::golem::worker::{
    self, worker_error, worker_execution_error, UnknownError, WorkerError as GrpcWorkerError,
};

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
    #[error("Bad Request: {0:?}")]
    BadRequest(Vec<String>),
    #[error("Already Exists: {0}")]
    AlreadyExists(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl ComponentServiceError {
    pub fn internal<M>(error: M) -> Self
    where
        M: std::fmt::Display + std::fmt::Debug + Send + Sync + 'static,
    {
        Self::Internal(anyhow::Error::msg(error))
    }
}

impl From<Status> for ComponentServiceError {
    fn from(status: Status) -> Self {
        ComponentServiceError::Internal(status.into())
    }
}

impl From<tonic::transport::Error> for ComponentServiceError {
    fn from(error: tonic::transport::Error) -> Self {
        ComponentServiceError::Internal(error.into())
    }
}

impl From<golem_api_grpc::proto::golem::component::ComponentError> for ComponentServiceError {
    fn from(error: golem_api_grpc::proto::golem::component::ComponentError) -> Self {
        use golem_api_grpc::proto::golem::component::component_error::Error;
        match error.error {
            Some(Error::BadRequest(errors)) => ComponentServiceError::BadRequest(errors.errors),
            Some(Error::Unauthorized(error)) => ComponentServiceError::Unauthorized(error.error),
            Some(Error::LimitExceeded(error)) => ComponentServiceError::Forbidden(error.error),
            Some(Error::NotFound(error)) => ComponentServiceError::NotFound(error.error),
            Some(Error::AlreadyExists(error)) => ComponentServiceError::AlreadyExists(error.error),
            Some(Error::InternalError(error)) => {
                ComponentServiceError::Internal(anyhow::Error::msg(error.error))
            }
            None => ComponentServiceError::Internal(anyhow::Error::msg("Unknown error")),
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
                worker_error::Error::InternalError(worker::WorkerExecutionError {
                    error: Some(worker_execution_error::Error::Unknown(UnknownError {
                        details: error.to_string(),
                    })),
                })
            }
        }
    }
}
